//! 全局用户偏好：决定编辑器启动时的默认行为。
//!
//! 持久层走 `$HOME/.zom/config.toml`，结构对应 [`AppConfig`]：
//! 顶层按"面"分组（目前只有 `editor`；后续 `ai` / `theme` / `keymap`各占一组），每组里放该面的偏好。
//!
//! 加载只在 [`App::new_persistent`](crate::app::App::new_persistent)启动期发生一次；
//! 运行时不 watch 文件。修改偏好走命令路径——命令既翻转 kernel 上对应的运行时句柄（如 soft_wrap 的 `Rc<Cell<bool>>`），又调用 [`AppConfig::save`] 把新值落盘。
//!
//! 单测构造的 [`App::new`] 传 `None` 路径走纯内存模式，与 `RecentProjects`的双模式一致。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 顶层配置：按面分组，每组一个子结构。
///
/// `#[serde(default)]` 让旧版本配置文件缺字段时也能反序列化为默认值——
/// 新增字段不破坏老配置。
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct AppConfig {
    pub(crate) editor: EditorConfig,
}

/// 编辑面偏好。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct EditorConfig {
    /// 主编辑区是否默认开启软换行。运行时由 `editor.toggle_soft_wrap` 翻转 kernel 中的 `Rc<Cell<bool>>`；
    /// 该命令同时把新值写回本字段，并 flush 到 [`AppConfig::default_path`]。
    pub(crate) soft_wrap: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self { soft_wrap: true }
    }
}

impl AppConfig {
    /// 从磁盘加载；`None` 路径表示内存模式（测试用）。
    /// 文件不存在、解析失败均回退到默认配置（同时 stderr 打印诊断）。
    pub(crate) fn load(path: Option<&Path>) -> Self {
        path.map(read_from_file).unwrap_or_default()
    }

    /// 发行版默认落盘位置：`$HOME/.zom/config.toml`。
    pub(crate) fn default_path() -> Option<PathBuf> {
        Some(home_dir()?.join(".zom/config.toml"))
    }

    /// 把当前配置写盘。`None` 路径静默忽略（内存模式）。
    pub(crate) fn save(&self, path: Option<&Path>) {
        let Some(path) = path else {
            return;
        };
        if let Err(error) = write_to_file(path, self) {
            eprintln!("写入全局配置失败：{error}");
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn read_from_file(path: &Path) -> AppConfig {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return AppConfig::default(),
        Err(error) => {
            eprintln!("读取全局配置失败：{error}");
            return AppConfig::default();
        }
    };
    match toml::from_str::<AppConfig>(&text) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("解析全局配置失败：{error}");
            AppConfig::default()
        }
    }
}

fn write_to_file(path: &Path, config: &AppConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建配置目录 {}：{error}", parent.display()))?;
    }
    let text =
        toml::to_string_pretty(config).map_err(|error| format!("无法序列化配置：{error}"))?;
    fs::write(path, text).map_err(|error| format!("无法写入配置文件 {}：{error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("zom-config-{tag}-{}.toml", std::process::id()))
    }

    #[test]
    fn load_missing_file_returns_default() {
        let path = temp_path("missing");
        let _ = fs::remove_file(&path);
        let loaded = AppConfig::load(Some(&path));
        assert_eq!(loaded, AppConfig::default());
    }

    #[test]
    fn round_trip_through_disk_preserves_values() {
        let path = temp_path("roundtrip");
        let _ = fs::remove_file(&path);
        let mut config = AppConfig::default();
        config.editor.soft_wrap = true;
        config.save(Some(&path));

        let loaded = AppConfig::load(Some(&path));
        assert_eq!(loaded, config);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn load_with_none_path_returns_default() {
        assert_eq!(AppConfig::load(None), AppConfig::default());
    }
}
