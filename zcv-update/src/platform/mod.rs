//! 自动更新的应用布局与系统边界。
//!
//! 更新事务和清单流程只依赖这里提供的语义；
//! 平台模块负责当前进程定位、应用布局、产物解压和应用校验等系统差异，
//! 并实现 helper 在应用退出后所需的 `ApplyBackend` 原语。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use crate::UpdateTransaction;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInstallation {
    app_path: PathBuf,
    platform_key: &'static str,
}

impl UpdateInstallation {
    pub fn from_process_path(process_path: &Path) -> Result<Self> {
        platform::from_process_path(process_path)
    }

    pub fn app_path(&self) -> &Path {
        &self.app_path
    }

    pub fn helper_path(&self) -> PathBuf {
        update_helper_path(&self.app_path)
    }

    pub fn platform_key(&self) -> &'static str {
        self.platform_key
    }
}

pub(crate) fn application_executable_path(app: &Path) -> PathBuf {
    app.join(platform::APP_EXECUTABLE_RELATIVE_PATH)
}

fn update_helper_path(app: &Path) -> PathBuf {
    app.join(platform::HELPER_RELATIVE_PATH)
}

pub(crate) fn is_application_directory(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(platform::APP_DIRECTORY_NAME)
}

/// 一次事务中平台后端暂存的新版本与旧版本备份。
pub(crate) struct StagedUpdate {
    /// 切换前的新版本候选目录。
    pub candidate_path: PathBuf,
    /// 切换后旧版本所在路径；用于清理或回滚。
    pub previous_path: PathBuf,
}

/// helper 启动新版本后持有的进程句柄。
pub(crate) trait StartedProcess {
    /// 进程已退出时返回退出状态描述；仍在运行时返回 `None`。
    fn exit_status(&mut self) -> Result<Option<String>>;
    fn kill(&mut self);
    fn wait(&mut self);
}

impl StartedProcess for Child {
    fn exit_status(&mut self) -> Result<Option<String>> {
        Ok(self
            .try_wait()
            .context("无法查询新版本进程状态")?
            .map(|status| status.to_string()))
    }

    fn kill(&mut self) {
        let _ = Child::kill(self);
    }

    fn wait(&mut self) {
        let _ = Child::wait(self);
    }
}

/// 平台后端为更新事务提供的原语。
///
/// 共同流程（暂存校验→原子切换→启动→启动确认→清理或回滚）由
/// `crate::helper::run_apply` 拥有；后端只承载系统调用、路径布局与进程退出等待。
pub(crate) trait ApplyBackend {
    fn wait_for_process_exit(&self, pid: u32) -> Result<()>;

    /// 复制并校验新版本到候选目录，返回切换所需的路径布局。
    fn stage(&self, transaction: &UpdateTransaction) -> Result<StagedUpdate>;

    /// 用候选目录替换安装目录；失败时旧版本必须仍在安装目录。
    fn switch(&self, transaction: &UpdateTransaction, staged: &StagedUpdate) -> Result<()>;

    /// 切换到新版本后失败时，把旧版本恢复到安装目录。
    fn rollback(&self, transaction: &UpdateTransaction, staged: &StagedUpdate) -> Result<()>;

    /// 清理候选与备份残留；失败只影响残留文件，不影响事务结果。
    fn cleanup(&self, transaction: &UpdateTransaction, staged: &StagedUpdate);

    fn launch(&self, app: &Path, update: Option<(&str, &Path)>) -> Result<Box<dyn StartedProcess>> {
        launch_application(app, update)
    }

    fn startup_timeout(&self) -> Duration {
        Duration::from_secs(30)
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_millis(100)
    }
}

/// 启动指定应用目录；可选注入启动确认所需的进程环境变量。
pub(crate) fn launch_application(
    app: &Path,
    update: Option<(&str, &Path)>,
) -> Result<Box<dyn StartedProcess>> {
    let executable = application_executable_path(app);
    let mut command = Command::new(&executable);
    command
        .current_dir(app)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some((transaction_id, ack_path)) = update {
        command.env("ZCV_UPDATE_TRANSACTION_ID", transaction_id);
        command.env("ZCV_UPDATE_ACK_PATH", ack_path);
    }
    let child = command
        .spawn()
        .with_context(|| format!("无法启动 {}", executable.display()))?;
    Ok(Box::new(child))
}

#[cfg(target_os = "macos")]
pub(crate) fn backend() -> impl ApplyBackend {
    macos::MacosBackend
}

#[cfg(target_os = "windows")]
pub(crate) fn backend() -> impl ApplyBackend {
    windows::WindowsBackend
}

pub(crate) use platform::{
    APP_DIRECTORY_NAME, archive_entry_is_allowed, extract_archive, replace_file,
    validate_extracted_app,
};
pub use platform::{prepare_helper, verify_app};

#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use std::path::Path;

    use anyhow::{Result, bail};
    use semver::Version;

    pub const APP_DIRECTORY_NAME: &str = "Zcv";
    pub const APP_EXECUTABLE_RELATIVE_PATH: &str = "Zcv";
    pub const HELPER_RELATIVE_PATH: &str = "zcv-update-helper";

    pub fn from_process_path(_: &Path) -> Result<super::UpdateInstallation> {
        bail!("当前平台尚不支持应用自动更新")
    }

    pub fn archive_entry_is_allowed(_: &str) -> bool {
        false
    }

    pub fn validate_extracted_app(_: &Path) -> Result<()> {
        bail!("当前平台尚不支持应用自动更新")
    }

    pub fn prepare_helper(_: &Path) -> Result<()> {
        bail!("当前平台尚不支持应用自动更新")
    }

    pub fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
        std::fs::rename(temporary, destination)?;
        Ok(())
    }

    pub fn verify_app(_: &Path, _: &Version) -> Result<()> {
        bail!("当前平台尚不支持应用自动更新")
    }

    pub async fn extract_archive(_: &Path, _: &Path, _: Vec<u8>) -> Result<()> {
        bail!("当前平台尚不支持应用自动更新")
    }
}
