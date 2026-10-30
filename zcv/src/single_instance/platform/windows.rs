//! Windows 启动请求转发。首个进程持有互斥体并接收后续进程的打开请求。

use std::{
    ffi::OsString,
    io,
    os::windows::ffi::{OsStrExt as _, OsStringExt as _},
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow, ensure};
use futures::channel::mpsc::UnboundedSender;
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE,
            GetLastError, HANDLE,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE, OPEN_EXISTING,
            PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
        },
        System::{
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
                PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
            },
            Threading::CreateMutexW,
        },
    },
    core::HSTRING,
};

use crate::InitialOpen;

const MUTEX_NAME: &str = "Local\\Zcv-Instance";
const PIPE_NAME: &str = "\\\\.\\pipe\\Zcv-Open-Request";

pub(crate) struct PrimaryInstance(HANDLE);

impl Drop for PrimaryInstance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0).ok();
        }
    }
}

/// 首个进程继续启动；后续进程等待首个进程接收请求后退出。
pub(crate) fn start(request: &InitialOpen) -> Result<Option<PrimaryInstance>> {
    let mutex = unsafe { CreateMutexW(None, false, &HSTRING::from(MUTEX_NAME)) }
        .context("无法建立 Zcv 实例互斥体")?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            CloseHandle(mutex).ok();
        }
        send_request(request)?;
        Ok(None)
    } else {
        Ok(Some(PrimaryInstance(mutex)))
    }
}

impl PrimaryInstance {
    pub(crate) fn listen(&self, sender: UnboundedSender<InitialOpen>) {
        std::thread::Builder::new()
            .name("zcv-open-requests".into())
            .spawn(move || {
                if let Err(error) = listen(sender) {
                    eprintln!("接收文件打开请求失败：{error:#}");
                }
            })
            .expect("文件打开请求监听线程应能启动");
    }
}

fn listen(sender: UnboundedSender<InitialOpen>) -> Result<()> {
    let name = HSTRING::from(PIPE_NAME);
    let pipe = unsafe {
        CreateNamedPipeW(
            &name,
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            65536,
            65536,
            0,
            None,
        )
    };
    ensure!(
        !pipe.is_invalid(),
        "无法创建文件打开请求管道：{}",
        io::Error::last_os_error()
    );
    loop {
        let connected = unsafe { ConnectNamedPipe(pipe, None) };
        if connected.is_err() && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            unsafe {
                CloseHandle(pipe).ok();
            }
            return Err(io::Error::last_os_error()).context("无法连接文件打开请求管道");
        }
        let result = read_request(pipe).and_then(|request| {
            sender
                .unbounded_send(request)
                .map_err(|_| anyhow!("应用已停止接收打开请求"))?;
            write_all(pipe, &[1])
        });
        unsafe {
            DisconnectNamedPipe(pipe).ok();
        }
        if let Err(error) = result {
            eprintln!("处理文件打开请求失败：{error:#}");
        }
    }
}

fn send_request(request: &InitialOpen) -> Result<()> {
    let name = HSTRING::from(PIPE_NAME);
    let started = Instant::now();
    let pipe = loop {
        if unsafe { WaitNamedPipeW(&name, 100) }.as_bool()
            && let Ok(pipe) = unsafe {
                CreateFileW(
                    &name,
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_MODE(0),
                    None,
                    OPEN_EXISTING,
                    FILE_FLAGS_AND_ATTRIBUTES(0),
                    None,
                )
            }
        {
            break pipe;
        }
        ensure!(
            started.elapsed() < Duration::from_secs(30),
            "无法连接现有 Zcv 进程：{}",
            io::Error::last_os_error()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let result = (|| {
        write_all(pipe, &encode_request(request)?)?;
        let mut ack = [0];
        read_exact(pipe, &mut ack)?;
        ensure!(ack == [1], "现有 Zcv 进程未确认打开请求");
        Ok(())
    })();
    unsafe {
        CloseHandle(pipe).ok();
    }
    result
}

fn encode_request(request: &InitialOpen) -> Result<Vec<u8>> {
    let (kind, path) = match request {
        InitialOpen::Empty => (0, None),
        InitialOpen::File(path) => (1, Some(path)),
        InitialOpen::Project(path) => (2, Some(path)),
    };
    let units: Vec<u16> = match path {
        Some(path) => std::path::absolute(path)
            .with_context(|| format!("无法解析启动路径：{}", path.display()))?
            .as_os_str()
            .encode_wide()
            .collect(),
        None => Vec::new(),
    };
    let length = u32::try_from(units.len()).context("文件路径过长")?;
    let mut bytes = Vec::with_capacity(5 + units.len() * 2);
    bytes.push(kind);
    bytes.extend_from_slice(&length.to_le_bytes());
    for unit in units {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(bytes)
}

fn read_request(pipe: HANDLE) -> Result<InitialOpen> {
    let mut header = [0; 5];
    read_exact(pipe, &mut header)?;
    let length = u32::from_le_bytes(header[1..5].try_into().unwrap()) as usize;
    ensure!(length <= 32767, "文件路径过长");
    let mut bytes = vec![0; length * 2];
    read_exact(pipe, &mut bytes)?;
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bytes| u16::from_le_bytes(*bytes))
        .collect();
    decode_request(header[0], &units)
}

fn decode_request(kind: u8, units: &[u16]) -> Result<InitialOpen> {
    let path = PathBuf::from(OsString::from_wide(units));
    match kind {
        0 if units.is_empty() => Ok(InitialOpen::Empty),
        1 if !units.is_empty() => Ok(InitialOpen::File(path)),
        2 if !units.is_empty() => Ok(InitialOpen::Project(path)),
        _ => Err(anyhow!("无效的文件打开请求")),
    }
}

fn read_exact(pipe: HANDLE, buffer: &mut [u8]) -> Result<()> {
    let mut offset = 0;
    while offset < buffer.len() {
        let mut count = 0;
        unsafe { ReadFile(pipe, Some(&mut buffer[offset..]), Some(&mut count), None) }
            .context("读取文件打开请求失败")?;
        ensure!(count > 0, "文件打开请求提前结束");
        offset += count as usize;
    }
    Ok(())
}

fn write_all(pipe: HANDLE, buffer: &[u8]) -> Result<()> {
    let mut offset = 0;
    while offset < buffer.len() {
        let mut count = 0;
        unsafe { WriteFile(pipe, Some(&buffer[offset..]), Some(&mut count), None) }
            .context("发送文件打开请求失败")?;
        ensure!(count > 0, "文件打开请求未发送");
        offset += count as usize;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../test/single_instance_tests.rs"]
mod tests;
