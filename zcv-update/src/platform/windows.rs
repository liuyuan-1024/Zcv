use std::{os::windows::ffi::OsStrExt as _, path::Path};

use anyhow::{Context as _, Result, ensure};
use async_zip::base::read::mem::ZipFileReader;
use futures::io::AsyncWriteExt as _;
use semver::Version;
use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS, GetLastError};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows_sys::Win32::System::RestartManager::{
    CCH_RM_SESSION_KEY, RmEndSession, RmGetList, RmRegisterResources, RmShutdown, RmStartSession,
};

use super::UpdateInstallation;

pub const APP_DIRECTORY_NAME: &str = "Zcv";
pub const APP_EXECUTABLE_RELATIVE_PATH: &str = "Zcv.exe";
pub const HELPER_RELATIVE_PATH: &str = "zcv-update-helper.exe";

pub fn from_process_path(process_path: &Path) -> Result<UpdateInstallation> {
    ensure!(
        process_path.file_name().and_then(|name| name.to_str()) == Some("Zcv.exe"),
        "当前进程不是 Zcv.exe"
    );
    let app_path = process_path
        .parent()
        .context("Zcv.exe 没有安装目录")?
        .to_path_buf();
    ensure!(
        app_path.file_name().and_then(|name| name.to_str()) == Some(APP_DIRECTORY_NAME),
        "当前安装目录不是 Zcv"
    );
    Ok(UpdateInstallation {
        app_path,
        platform_key: platform_key()?,
    })
}

pub fn archive_entry_is_allowed(name: &str) -> bool {
    name == APP_DIRECTORY_NAME || name.starts_with(&format!("{APP_DIRECTORY_NAME}/"))
}

pub fn validate_extracted_app(app: &Path) -> Result<()> {
    ensure!(
        app.join(APP_EXECUTABLE_RELATIVE_PATH).is_file(),
        "更新包缺少 Zcv.exe"
    );
    ensure!(
        app.join(HELPER_RELATIVE_PATH).is_file(),
        "更新包缺少更新辅助程序"
    );
    ensure!(app.join("version.txt").is_file(), "更新包缺少 version.txt");
    Ok(())
}

pub fn prepare_helper(_: &Path) -> Result<()> {
    Ok(())
}

/// 请求 Windows 释放 Explorer、索引器等进程对安装文件的占用。
///
/// Windows 允许外部进程短暂持有应用目录中的文件；
/// 这些句柄会使后续目录重命名失败。
/// Restart Manager 只负责尽力释放句柄，真正的替换仍由 helper 的事务回滚流程负责。
pub(super) fn release_file_handles(app: &Path) -> Result<()> {
    let paths = [
        app.join(APP_EXECUTABLE_RELATIVE_PATH),
        app.join(HELPER_RELATIVE_PATH),
        app.join("version.txt"),
    ];
    let wide_paths = paths
        .iter()
        .map(|path| {
            path.as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let resource_paths = wide_paths
        .iter()
        .map(|path| path.as_ptr())
        .collect::<Vec<_>>();

    let mut session = 0_u32;
    let mut session_key = [0_u16; CCH_RM_SESSION_KEY as usize + 1];
    let start_result = unsafe { RmStartSession(&mut session, 0, session_key.as_mut_ptr()) };
    ensure!(
        start_result == ERROR_SUCCESS,
        "无法启动 Windows Restart Manager 会话：错误码 {start_result}"
    );

    let result = (|| {
        let register_result = unsafe {
            RmRegisterResources(
                session,
                resource_paths.len() as u32,
                resource_paths.as_ptr(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
            )
        };
        ensure!(
            register_result == ERROR_SUCCESS,
            "无法注册 Windows 更新文件：错误码 {register_result}"
        );

        let mut needed = 0_u32;
        let mut listed = 0_u32;
        let mut reboot_reasons = 0_u32;
        let list_result = unsafe {
            RmGetList(
                session,
                &mut needed,
                &mut listed,
                std::ptr::null_mut(),
                &mut reboot_reasons,
            )
        };
        ensure!(
            list_result == ERROR_SUCCESS || list_result == ERROR_MORE_DATA,
            "无法查询占用 Windows 更新文件的进程：错误码 {list_result}"
        );
        if needed == 0 {
            return Ok(());
        }

        let shutdown_result = unsafe { RmShutdown(session, 0, None) };
        ensure!(
            shutdown_result == ERROR_SUCCESS,
            "无法请求 Windows 释放更新文件：错误码 {shutdown_result}"
        );
        Ok(())
    })();
    unsafe { RmEndSession(session) };
    result
}

pub fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
    let temporary = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_path = destination.to_path_buf();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    ensure!(
        replaced != 0,
        "无法原子提交文件 {}：Windows 错误码 {}",
        destination_path.display(),
        unsafe { GetLastError() }
    );
    Ok(())
}

pub fn verify_app(app: &Path, expected_version: &Version) -> Result<()> {
    validate_extracted_app(app)?;
    let version =
        std::fs::read_to_string(app.join("version.txt")).context("无法读取更新应用版本")?;
    ensure!(
        version.trim() == expected_version.to_string(),
        "更新应用版本与清单目标版本不一致"
    );
    Ok(())
}

pub async fn extract_archive(_archive: &Path, destination: &Path, bytes: Vec<u8>) -> Result<()> {
    let reader = ZipFileReader::new(bytes).await?;
    for index in 0..reader.file().entries().len() {
        let entry = &reader.file().entries()[index];
        let name = entry
            .filename()
            .as_str()
            .context("更新压缩包包含非 UTF-8 路径")?;
        let path = destination.join(name);
        if entry.dir().context("无法读取更新压缩包目录项")? {
            smol::fs::create_dir_all(&path).await?;
            continue;
        }
        let parent = path.parent().context("更新压缩包文件没有父目录")?;
        smol::fs::create_dir_all(parent).await?;
        let mut entry_reader = reader.reader_without_entry(index).await?;
        let mut output = smol::fs::File::create(&path).await?;
        futures::io::copy(&mut entry_reader, &mut output).await?;
        output.flush().await?;
    }
    Ok(())
}

fn platform_key() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("windows-x86_64"),
        "aarch64" => Ok("windows-aarch64"),
        architecture => anyhow::bail!("不支持 Windows 自动更新架构 {architecture}"),
    }
}
