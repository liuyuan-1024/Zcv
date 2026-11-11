//! 设置文件路径与创建。
//!
//! 路径从 `config_dir()` 派生；用户配置只保存覆盖值。

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use anyhow::{Context as _, Result};

use super::{config_dir, schema::UserSettingsContent};

pub(crate) const USER_SETTINGS_FILE_HEADER: &str =
    "// 设置可在 Zcv 设置窗口中修改；此文件只保存显式覆盖值。";

pub(crate) fn settings_file() -> &'static Path {
    static SETTINGS_FILE: OnceLock<PathBuf> = OnceLock::new();
    SETTINGS_FILE
        .get_or_init(|| config_dir().join("settings.json"))
        .as_path()
}

/// 用户设置文件的稳定路径；设置窗口内嵌编辑器从这一入口读取文件。
pub fn settings_file_path() -> &'static Path {
    settings_file()
}

/// 读取设置文件原文；内嵌编辑器需要保留原始 JSONC 内容。
pub fn read_user_settings_text() -> Result<String> {
    read_user_settings_file_at(settings_file())
}

/// 写回设置文件原文；手工编辑与内嵌编辑器共用原始文本写入。
pub fn write_user_settings_text(content: &str) -> Result<()> {
    write_user_settings_file_at(settings_file(), content)
}

/// 确保用户设置文件存在，返回用户设置文件路径。
pub fn ensure_user_settings_file() -> Result<PathBuf> {
    let user = settings_file();
    ensure_settings_file(user, &format!("{USER_SETTINGS_FILE_HEADER}\n{{}}\n"))?;
    Ok(user.to_path_buf())
}

/// 读取指定用户设置文件；文件不存在时返回空文本，由解析层按“无覆盖”处理。
pub(crate) fn read_user_settings_file_at(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("无法读取设置文件 {}", path.display())),
    }
}

pub(crate) fn write_user_settings_file_at(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建设置目录 {}", parent.display()))?;
    }
    fs::write(path, content).with_context(|| format!("无法写入设置文件 {}", path.display()))
}

/// 把面板维护的用户覆盖值序列化为 settings.json 文本。
///
/// 只写入显式覆盖的字段；反序列化与校验仍由 settings 层统一完成。
pub(crate) fn serialize_user_settings(content: &UserSettingsContent) -> Result<String> {
    let body = serde_json_lenient::to_string_pretty(content).context("无法序列化用户设置")?;
    Ok(format!("{USER_SETTINGS_FILE_HEADER}\n{body}\n"))
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
