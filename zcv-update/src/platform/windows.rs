use std::fs;
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, ensure};
use async_zip::base::read::mem::ZipFileReader;
use futures::io::AsyncWriteExt as _;
use semver::Version;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, ERROR_SUCCESS, GetLastError,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows_sys::Win32::System::RestartManager::{
    CCH_RM_SESSION_KEY, RmEndSession, RmGetList, RmRegisterResources, RmShutdown, RmStartSession,
};
use windows_sys::Win32::System::Threading::{
    INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

use super::{ApplyBackend, StagedUpdate, UpdateInstallation};
use crate::UpdateTransaction;

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
fn release_file_handles(app: &Path) -> Result<()> {
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

/// Windows helper 后端：备份/重命名切换与进程退出等待。
pub(crate) struct WindowsBackend;

impl ApplyBackend for WindowsBackend {
    fn wait_for_process_exit(&self, pid: u32) -> Result<()> {
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            ensure!(
                unsafe { GetLastError() } == ERROR_INVALID_PARAMETER,
                "无法打开 Zcv 进程以等待退出"
            );
            return Ok(());
        }
        let result = unsafe { WaitForSingleObject(handle, INFINITE) };
        unsafe { CloseHandle(handle) };
        ensure!(result == WAIT_OBJECT_0, "等待 Zcv 退出失败");
        Ok(())
    }

    fn stage(&self, transaction: &UpdateTransaction) -> Result<StagedUpdate> {
        verify_app(&transaction.staged_app_path, &transaction.to_version)?;
        if let Err(error) = release_file_handles(&transaction.install_path) {
            eprintln!("无法释放 Windows 更新文件占用，将继续尝试替换：{error:#}");
        }
        let candidate_path = candidate_path(transaction)?;
        let backup_path = backup_path(transaction)?;
        if candidate_path.exists() {
            fs::remove_dir_all(&candidate_path)
                .with_context(|| format!("无法清理旧更新候选目录 {}", candidate_path.display()))?;
        }
        copy_dir(&transaction.staged_app_path, &candidate_path)?;
        let candidate_transaction = UpdateTransaction {
            staged_app_path: candidate_path.clone(),
            ..transaction.clone()
        };
        if let Err(error) = verify_app(
            &candidate_transaction.staged_app_path,
            &candidate_transaction.to_version,
        ) {
            let _ = fs::remove_dir_all(&candidate_path);
            return Err(error).context("安装目录中的更新副本验证失败");
        }
        Ok(StagedUpdate {
            candidate_path,
            previous_path: backup_path,
        })
    }

    fn switch(&self, transaction: &UpdateTransaction, staged: &StagedUpdate) -> Result<()> {
        let backup_path = &staged.previous_path;
        if backup_path.exists() {
            fs::remove_dir_all(backup_path)
                .with_context(|| format!("无法清理旧版本备份 {}", backup_path.display()))?;
        }
        retry_rename(
            &transaction.install_path,
            backup_path,
            format!(
                "无法移动当前安装目录 {} → {}",
                transaction.install_path.display(),
                backup_path.display()
            ),
        )?;
        if let Err(error) = retry_rename(
            &staged.candidate_path,
            &transaction.install_path,
            format!(
                "无法把更新目录移动到安装位置 {}",
                transaction.install_path.display()
            ),
        ) {
            let _ = retry_rename(
                backup_path,
                &transaction.install_path,
                format!(
                    "无法恢复当前安装目录 {}",
                    transaction.install_path.display()
                ),
            );
            return Err(error).context("无法把更新目录移动到安装位置");
        }
        Ok(())
    }

    fn rollback(&self, transaction: &UpdateTransaction, staged: &StagedUpdate) -> Result<()> {
        fs::remove_dir_all(&transaction.install_path)
            .context("新版本启动失败，且无法移除新版本")?;
        retry_rename(
            &staged.previous_path,
            &transaction.install_path,
            "新版本启动失败，且旧版本回滚失败".to_owned(),
        )
    }

    fn cleanup(&self, _transaction: &UpdateTransaction, staged: &StagedUpdate) {
        if staged.previous_path.exists()
            && let Err(error) = fs::remove_dir_all(&staged.previous_path)
        {
            eprintln!(
                "新版本已启动，但无法删除旧版本备份 {}：{error}",
                staged.previous_path.display()
            );
        }
        let _ = fs::remove_dir_all(&staged.candidate_path);
    }
}

fn retry_rename(from: &Path, to: &Path, context: String) -> Result<()> {
    const RETRY_TIMEOUT: Duration = Duration::from_secs(5);
    const POLL_INTERVAL: Duration = Duration::from_millis(100);
    let start = Instant::now();
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(_error) if start.elapsed() < RETRY_TIMEOUT => {
                thread::sleep(POLL_INTERVAL);
            }
            Err(error) => return Err(error).context(context),
        }
    }
}

fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("无法创建更新目录 {}", destination.display()))?;
    for entry in
        fs::read_dir(source).with_context(|| format!("无法读取更新目录 {}", source.display()))?
    {
        let entry = entry.with_context(|| format!("无法读取目录项 {}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry
            .file_type()
            .with_context(|| format!("无法读取文件类型 {}", source_path.display()))?
            .is_dir()
        {
            copy_dir(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "无法复制更新文件 {} → {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn candidate_path(transaction: &UpdateTransaction) -> Result<PathBuf> {
    let parent = transaction
        .install_path
        .parent()
        .context("安装路径没有父目录")?;
    Ok(parent.join(format!(".Zcv.update-{}", transaction.id)))
}

fn backup_path(transaction: &UpdateTransaction) -> Result<PathBuf> {
    let parent = transaction
        .install_path
        .parent()
        .context("安装路径没有父目录")?;
    Ok(parent.join(format!(".Zcv.backup-{}", transaction.id)))
}
