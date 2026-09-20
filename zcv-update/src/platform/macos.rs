use std::ffi::{CString, OsStr};
use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use semver::Version;
use smol::process::Command;

use super::{ApplyBackend, StagedUpdate, UpdateInstallation};
use crate::UpdateTransaction;

pub const APP_DIRECTORY_NAME: &str = "Zcv.app";
pub const APP_EXECUTABLE_RELATIVE_PATH: &str = "Contents/MacOS/Zcv";
pub const HELPER_RELATIVE_PATH: &str = "Contents/Helpers/zcv-update-helper";

pub fn from_process_path(process_path: &Path) -> Result<UpdateInstallation> {
    ensure!(
        process_path.file_name().and_then(|name| name.to_str()) == Some(APP_DIRECTORY_NAME),
        "当前 app bundle 不是 Zcv.app"
    );
    ensure!(
        !is_translocated_path(process_path),
        "应用运行在 App Translocation 临时路径中，请把 Zcv.app 移到 /Applications 后重启"
    );
    Ok(UpdateInstallation {
        app_path: process_path.to_path_buf(),
        platform_key: platform_key()?,
    })
}

pub fn archive_entry_is_allowed(name: &str) -> bool {
    name == APP_DIRECTORY_NAME
        || name.starts_with(&format!("{APP_DIRECTORY_NAME}/"))
        || name == "__MACOSX"
        || name == "__MACOSX/"
        || name == "__MACOSX/._Zcv.app"
        || name.starts_with("__MACOSX/Zcv.app/")
}

pub fn validate_extracted_app(app: &Path) -> Result<()> {
    ensure!(
        app.join(APP_EXECUTABLE_RELATIVE_PATH).is_file(),
        "更新包缺少应用可执行文件"
    );
    ensure!(
        app.join(HELPER_RELATIVE_PATH).is_file(),
        "更新包缺少更新辅助程序"
    );
    ensure!(
        app.join("Contents/Info.plist").is_file(),
        "更新包缺少 Info.plist"
    );
    Ok(())
}

pub fn prepare_helper(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

pub fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
    fs::rename(temporary, destination)
        .with_context(|| format!("无法提交文件 {}", destination.display()))?;
    Ok(())
}

pub fn verify_app(app: &Path, expected_version: &Version) -> Result<()> {
    ensure!(
        app.join(APP_EXECUTABLE_RELATIVE_PATH).is_file(),
        "更新应用缺少可执行文件"
    );
    let verify = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(app)
        .output()
        .context("无法启动 codesign 验证更新")?;
    ensure!(
        verify.status.success(),
        "更新应用代码签名无效：{}",
        String::from_utf8_lossy(&verify.stderr).trim()
    );

    let version = std::process::Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print:CFBundleShortVersionString"])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .context("无法读取更新应用版本")?;
    ensure!(version.status.success(), "无法读取更新应用版本");
    ensure!(
        String::from_utf8_lossy(&version.stdout).trim() == expected_version.to_string(),
        "更新应用版本与清单目标版本不一致"
    );
    Ok(())
}

pub async fn extract_archive(archive: &Path, destination: &Path, _bytes: Vec<u8>) -> Result<()> {
    let output = Command::new("/usr/bin/ditto")
        .args([OsStr::new("-x"), OsStr::new("-k")])
        .arg(archive)
        .arg(destination)
        .stdout(Stdio::null())
        .output()
        .await
        .context("无法启动 ditto 解压更新")?;
    ensure!(
        output.status.success(),
        "ditto 解压更新失败：{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

pub fn is_translocated_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "AppTranslocation")
}

fn platform_key() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "aarch64" => Ok("macos-aarch64"),
        architecture => anyhow::bail!("不支持 macOS 自动更新架构 {architecture}"),
    }
}

/// macOS helper 后端：原子交换原语与进程退出等待。
pub(crate) struct MacosBackend;

impl ApplyBackend for MacosBackend {
    fn wait_for_process_exit(&self, pid: u32) -> Result<()> {
        loop {
            let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }

    fn stage(&self, transaction: &UpdateTransaction) -> Result<StagedUpdate> {
        verify_transaction_app(transaction)?;
        let candidate_path = candidate_path(transaction)?;
        if candidate_path.exists() {
            fs::remove_dir_all(&candidate_path)
                .with_context(|| format!("无法清理旧更新候选目录 {}", candidate_path.display()))?;
        }

        let copy = std::process::Command::new("/usr/bin/ditto")
            .arg(&transaction.staged_app_path)
            .arg(&candidate_path)
            .output()
            .context("无法启动 ditto 复制更新应用")?;
        ensure!(
            copy.status.success(),
            "无法把更新复制到安装目录：{}",
            String::from_utf8_lossy(&copy.stderr).trim()
        );

        let candidate_transaction = UpdateTransaction {
            staged_app_path: candidate_path.clone(),
            ..transaction.clone()
        };
        if let Err(error) = verify_transaction_app(&candidate_transaction) {
            let _ = fs::remove_dir_all(&candidate_path);
            return Err(error).context("安装目录中的更新副本验证失败");
        }
        strip_download_metadata(&candidate_path).context("无法清理安装副本的下载元数据")?;
        Ok(StagedUpdate {
            candidate_path: candidate_path.clone(),
            previous_path: candidate_path,
        })
    }

    fn switch(&self, transaction: &UpdateTransaction, staged: &StagedUpdate) -> Result<()> {
        atomic_swap(&transaction.install_path, &staged.candidate_path)
            .context("无法原子切换 Zcv.app")
    }

    fn rollback(&self, transaction: &UpdateTransaction, staged: &StagedUpdate) -> Result<()> {
        atomic_swap(&transaction.install_path, &staged.candidate_path).context("无法回滚 Zcv.app")
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
    }
}

fn candidate_path(transaction: &UpdateTransaction) -> Result<PathBuf> {
    let parent = transaction
        .install_path
        .parent()
        .context("Zcv.app 安装路径没有父目录")?;
    Ok(parent.join(format!(".Zcv.update-{}.app", transaction.id)))
}

fn verify_transaction_app(transaction: &UpdateTransaction) -> Result<()> {
    verify_app(&transaction.staged_app_path, &transaction.to_version)
}

/// 移除下载元数据（quarantine / provenance）。
///
/// 只作用于已经通过清单签名、SHA-256 与代码签名验证的副本；避免 Gatekeeper
/// 对已验证副本在每次更新后再次要求人工批准。normal 路径下副本通常没有这些
/// 属性，removexattr 以 ENOATTR 结束并被忽略。
fn strip_download_metadata(root: &Path) -> Result<()> {
    fn strip_one(path: &Path) -> Result<()> {
        for attribute in ["com.apple.quarantine", "com.apple.provenance"] {
            let c_path = CString::new(path.as_os_str().as_bytes()).context("路径包含空字节")?;
            let c_attribute = CString::new(attribute).context("属性名包含空字节")?;
            let result = unsafe { libc::removexattr(c_path.as_ptr(), c_attribute.as_ptr(), 0) };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ENOATTR) {
                    return Err(error).with_context(|| {
                        format!("无法移除 {} 的属性 {attribute}", path.display())
                    });
                }
            }
        }
        Ok(())
    }
    fn visit(path: &Path) -> Result<()> {
        for entry in
            fs::read_dir(path).with_context(|| format!("无法读取目录 {}", path.display()))?
        {
            let entry = entry.with_context(|| format!("无法读取目录项 {}", path.display()))?;
            let entry_path = entry.path();
            strip_one(&entry_path)?;
            if entry
                .file_type()
                .with_context(|| format!("无法读取文件类型 {}", entry_path.display()))?
                .is_dir()
            {
                visit(&entry_path)?;
            }
        }
        Ok(())
    }
    strip_one(root)?;
    visit(root)
}

fn atomic_swap(first: &Path, second: &Path) -> Result<()> {
    let first = CString::new(first.as_os_str().as_bytes()).context("安装路径包含空字节")?;
    let second = CString::new(second.as_os_str().as_bytes()).context("候选路径包含空字节")?;
    let result = unsafe { libc::renamex_np(first.as_ptr(), second.as_ptr(), libc::RENAME_SWAP) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("renamex_np(RENAME_SWAP) 失败");
    }
    Ok(())
}

#[cfg(test)]
#[path = "test/macos_tests.rs"]
mod tests;
