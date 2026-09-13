//! 自动更新的应用布局与系统边界。
//!
//! 更新事务和清单流程只依赖这里提供的语义；
//! 平台模块负责当前进程定位、应用布局、产物解压和应用校验等系统差异。

use std::path::{Path, PathBuf};

use anyhow::Result;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

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

pub fn application_executable_path(app: &Path) -> PathBuf {
    app.join(platform::APP_EXECUTABLE_RELATIVE_PATH)
}

pub fn update_helper_path(app: &Path) -> PathBuf {
    app.join(platform::HELPER_RELATIVE_PATH)
}

pub(crate) fn is_application_directory(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(platform::APP_DIRECTORY_NAME)
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
