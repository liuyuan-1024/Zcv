use std::path::PathBuf;

use anyhow::{Context as _, Result};
use gpui::BackgroundExecutor;
#[cfg(windows)]
use gpui_util::new_std_command;

#[derive(Clone, Copy)]
pub(crate) struct ProcessIdGetter {
    #[cfg(unix)]
    handle: i32,
    fallback_pid: u32,
}

impl ProcessIdGetter {
    #[cfg(unix)]
    pub(crate) fn new(handle: i32, fallback_pid: u32) -> Self {
        Self {
            handle,
            fallback_pid,
        }
    }

    #[cfg(windows)]
    pub(crate) fn new(fallback_pid: u32) -> Self {
        Self { fallback_pid }
    }

    pub(super) fn fallback_pid(&self) -> u32 {
        self.fallback_pid
    }

    #[cfg(unix)]
    pub(super) fn foreground_pid(&self) -> Option<u32> {
        let pid = unsafe { libc::tcgetpgrp(self.handle) };
        if pid > 0 {
            Some(pid as u32)
        } else if self.fallback_pid > 0 {
            Some(self.fallback_pid)
        } else {
            None
        }
    }

    #[cfg(windows)]
    pub(super) fn foreground_pid(&self) -> Option<u32> {
        (self.fallback_pid > 0).then_some(self.fallback_pid)
    }
}

pub(super) struct ProcessCwdReader {
    #[cfg(windows)]
    system: sysinfo::System,
    #[cfg(windows)]
    refresh_kind: sysinfo::ProcessRefreshKind,
    #[cfg(windows)]
    last_pid: Option<sysinfo::Pid>,
}

impl ProcessCwdReader {
    pub(super) fn new() -> Self {
        Self {
            #[cfg(windows)]
            system: sysinfo::System::new(),
            #[cfg(windows)]
            refresh_kind: sysinfo::ProcessRefreshKind::nothing()
                .with_cwd(sysinfo::UpdateKind::Always)
                .without_tasks(),
            #[cfg(windows)]
            last_pid: None,
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn read_cwd(&mut self, pid: u32) -> Option<PathBuf> {
        use std::{
            ffi::{OsStr, c_void},
            mem::{MaybeUninit, size_of, size_of_val},
            os::unix::ffi::OsStrExt as _,
        };

        let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
        let buffer_size = size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
        let written = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                info.as_mut_ptr().cast::<c_void>(),
                buffer_size,
            )
        };
        if written != buffer_size {
            return None;
        }
        let info = unsafe { info.assume_init() };
        let raw_bytes = unsafe {
            std::slice::from_raw_parts(
                info.pvi_cdir.vip_path.as_ptr().cast::<u8>(),
                size_of_val(&info.pvi_cdir.vip_path),
            )
        };
        let end = raw_bytes
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(raw_bytes.len());
        if end == 0 {
            return None;
        }
        Some(PathBuf::from(OsStr::from_bytes(&raw_bytes[..end])))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    pub(super) fn read_cwd(&mut self, pid: u32) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }

    #[cfg(windows)]
    pub(super) fn read_cwd(&mut self, pid: u32) -> Option<PathBuf> {
        let pid = sysinfo::Pid::from_u32(pid);
        if self.last_pid.replace(pid) != Some(pid) {
            self.system = sysinfo::System::new();
        }
        self.system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            self.refresh_kind,
        );
        self.system.process(pid)?.cwd().map(PathBuf::from)
    }
}

pub(super) fn terminate_process_tree(pid: u32, executor: &BackgroundExecutor) -> Result<()> {
    #[cfg(unix)]
    {
        use std::{io, time::Duration};

        const PROCESS_KILL_GRACE_PERIOD: Duration = Duration::from_millis(100);
        let pid = pid as i32;
        // 先终止进程组，宽限期后再强制终止。
        let result = unsafe { libc::killpg(pid, libc::SIGTERM) };
        if result != 0 {
            let error = io::Error::last_os_error();
            // 进程在请求关闭前自然退出时，目标已经达到。
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error).context("发送终端进程组终止信号失败");
            }
        }
        let timer = executor.clone();
        executor
            .spawn(async move {
                timer.timer(PROCESS_KILL_GRACE_PERIOD).await;
                let result = unsafe { libc::killpg(pid, libc::SIGKILL) };
                if result != 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        eprintln!("终端进程组强制终止失败：{error}");
                    }
                }
            })
            .detach();
    }

    #[cfg(windows)]
    {
        let _ = executor;
        // ConPTY 不会因关闭事件循环而可靠终止 shell 的子进程树。
        let status = new_std_command("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .context("执行 taskkill 终止终端进程树失败")?;
        if !status.success() {
            anyhow::bail!("taskkill 终止终端进程树返回状态 {status}");
        }
    }

    Ok(())
}
