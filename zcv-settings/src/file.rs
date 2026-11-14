//! 用户设置文件的路径、迁移与写入。
//!
//! 路径从 `config_dir()` 派生；用户配置只保存覆盖值。

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use anyhow::{Context as _, Result};

use super::config_dir;

const USER_SETTINGS_FILE_HEADER: &str = "// 设置可在 Zcv 设置窗口中修改；此文件只保存显式覆盖值。";

pub(crate) fn settings_file() -> &'static Path {
    static SETTINGS_FILE: OnceLock<PathBuf> = OnceLock::new();
    SETTINGS_FILE
        .get_or_init(|| config_dir().join("settings.jsonc"))
        .as_path()
}

/// 确保用户设置文件存在，返回用户设置文件路径。
pub fn ensure_user_settings_file() -> Result<PathBuf> {
    let user = settings_file();
    prepare_user_settings_file(user)?;
    Ok(user.to_path_buf())
}

pub(crate) fn prepare_user_settings_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("无法检查设置文件 {}", path.display()));
        }
    }
    let legacy = path.with_file_name("settings.json");
    match fs::rename(&legacy, path) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法将 {} 迁移到 {}", legacy.display(), path.display()));
        }
    }
    ensure_settings_file(path, &format!("{USER_SETTINGS_FILE_HEADER}\n{{}}\n"))
}

/// 读取指定用户设置文件；文件不存在时返回空文本，由解析层按“无覆盖”处理。
pub(crate) fn read_user_settings_file_at(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("无法读取设置文件 {}", path.display())),
    }
}

/// 在目标所在目录暂存并原子替换；已有符号链接继续指向原目标。
pub(crate) fn replace_user_settings_file(path: &Path, content: &str) -> Result<()> {
    let (target, permissions) = match fs::canonicalize(path) {
        Ok(target) => {
            let permissions = fs::metadata(&target)?.permissions();
            (target, Some(permissions))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // 断开的符号链接也必须保留，不能被原子替换成普通文件。
            match fs::symlink_metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    (path.to_path_buf(), None)
                }
                _ => {
                    return Err(error)
                        .with_context(|| format!("无法解析设置文件 {}", path.display()));
                }
            }
        }
        Err(error) => {
            return Err(error).with_context(|| format!("无法解析设置文件 {}", path.display()));
        }
    };
    let parent = target.parent().context("设置文件缺少父目录")?;
    fs::create_dir_all(parent).with_context(|| format!("无法创建设置目录 {}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("无法在 {} 中暂存设置", parent.display()))?;
    temporary
        .write_all(content.as_bytes())
        .context("无法写入临时设置文件")?;
    if let Some(permissions) = permissions {
        temporary
            .as_file()
            .set_permissions(permissions)
            .context("无法保留设置文件权限")?;
    }
    temporary
        .as_file()
        .sync_all()
        .context("无法同步临时设置文件")?;
    temporary
        .persist(&target)
        .map_err(|error| error.error)
        .with_context(|| format!("无法替换设置文件 {}", target.display()))?;
    Ok(())
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
