//! 设置文件路径与创建。
//!
//! 路径从 `config_dir()` 派生；这里只确保文件存在，不解析内容。

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result};

use super::{INITIAL_USER_SETTINGS, config_dir};

pub(crate) fn settings_file() -> &'static Path {
    static SETTINGS_FILE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    SETTINGS_FILE
        .get_or_init(|| config_dir().join("settings.json"))
        .as_path()
}

pub fn ensure_user_settings_file() -> Result<&'static Path> {
    let path = settings_file();
    ensure_settings_file(path, &INITIAL_USER_SETTINGS)?;
    Ok(path)
}

pub(crate) fn ensure_settings_file(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().context("设置文件缺少父目录")?;
    fs::create_dir_all(parent).with_context(|| format!("无法创建设置目录 {}", parent.display()))?;
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => file
            .write_all(content.as_bytes())
            .with_context(|| format!("无法写入设置文件 {}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| format!("无法创建设置文件 {}", path.display()));
        }
    }
    Ok(())
}
