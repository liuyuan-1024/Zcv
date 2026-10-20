//! 设置文件路径与创建。
//!
//! 路径从 `config_dir()` 派生；用户配置只保存覆盖值，参考文件随内置配置更新。

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use super::{INITIAL_USER_SETTINGS, config_dir};

pub(crate) fn settings_file() -> &'static Path {
    static SETTINGS_FILE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    SETTINGS_FILE
        .get_or_init(|| config_dir().join("settings.json"))
        .as_path()
}

/// 确保用户配置与当前版本的参考文件存在，按参考文件、用户文件顺序返回路径。
pub fn ensure_settings_files() -> Result<(PathBuf, PathBuf)> {
    let user = settings_file();
    ensure_settings_file(
        user,
        "// 配置项和当前默认值见同目录的 settings.reference.jsonc。\n{}\n",
    )?;
    let reference = config_dir().join("settings.reference.jsonc");
    update_reference_file(&reference, &INITIAL_USER_SETTINGS)?;
    Ok((reference, user.to_path_buf()))
}

fn update_reference_file(path: &Path, content: &str) -> Result<()> {
    if fs::read(path).is_ok_and(|existing| existing == content.as_bytes()) {
        return Ok(());
    }
    fs::write(path, content).with_context(|| format!("无法更新配置参考文件 {}", path.display()))
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

#[cfg(test)]
#[path = "test/file_tests.rs"]
mod tests;
