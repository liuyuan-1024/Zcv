//! 全局用户偏好：决定编辑器启动时的默认行为。
//!
//! 持久层走 `$HOME/.zom/config.toml`，结构对应 [`AppConfig`]：
//! 顶层按"面"分组（目前有 `general` / `ui` / `editor`；后续 `ai` / `keymap`各占一组），每组里放该面的偏好。
//!
//! 加载只在 [`App::new_persistent`](crate::app::App::new_persistent)启动期发生一次；
//! 运行时不 watch 文件。修改偏好走命令路径——命令既翻转 kernel 上对应的运行时句柄（如 soft_wrap 的 `Rc<Cell<bool>>`），又调用 [`AppConfig::save`] 把新值落盘。
//!
//! 单测构造的 [`App::new`] 传 `None` 路径走纯内存模式，与 `RecentProjects`的双模式一致。

use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zom_engine::{BufferConfig, TabConfig};

/// 顶层配置：按面分组，每组一个子结构。
///
/// `#[serde(default)]` 让旧版本配置文件缺字段时也能反序列化为默认值——
/// 新增字段不破坏老配置。
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct AppConfig {
    pub(crate) general: GeneralConfig,
    pub(crate) ui: UiConfig,
    pub(crate) editor: EditorConfig,
}

/// 全局偏好。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct GeneralConfig {
    /// 主题标识。当前语法高亮先消费它；完整 UI 主题后续接同一个字段。
    pub(crate) theme: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            theme: THEME_ONE_DARK.to_string(),
        }
    }
}

/// 界面偏好。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct UiConfig {
    /// UI chrome 字号，单位 px。
    pub(crate) font_size: u16,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { font_size: 13 }
    }
}

/// 编辑面偏好。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct EditorConfig {
    /// 多行编辑器（主编辑区 + 所有嵌入式多行编辑器）是否默认开启软换行。
    /// 运行时由 `editor.toggle_soft_wrap` 翻转 App 持有的共享 `Rc<Cell<bool>>`，
    /// 同帧同步到所有多行 kernel；命令同时把新值写回本字段并 flush 到磁盘。
    pub(crate) soft_wrap: bool,
    /// 编辑区代码字号，单位 px。
    pub(crate) font_size: u16,
    /// Tab 的视觉宽度，同时作为默认缩进宽度。
    pub(crate) tab_size: u16,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            soft_wrap: true,
            font_size: 16,
            tab_size: 4,
        }
    }
}

pub(crate) const THEME_ONE_DARK: &str = "one-dark";

const UI_FONT_MIN: u16 = 11;
const UI_FONT_MAX: u16 = 18;
const EDITOR_FONT_MIN: u16 = 12;
const EDITOR_FONT_MAX: u16 = 26;
const TAB_SIZES: [u16; 4] = [2, 4, 6, 8];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettingsChange {
    AdjustUiFont(i16),
    AdjustEditorFont(i16),
    ToggleEditorSoftWrap,
    CycleEditorTabSize,
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

    pub(crate) fn apply_change(&mut self, change: SettingsChange) {
        match change {
            SettingsChange::AdjustUiFont(delta) => {
                self.ui.font_size = stepped(self.ui.font_size, delta, UI_FONT_MIN, UI_FONT_MAX);
            }
            SettingsChange::AdjustEditorFont(delta) => {
                self.editor.font_size = stepped(
                    self.editor.font_size,
                    delta,
                    EDITOR_FONT_MIN,
                    EDITOR_FONT_MAX,
                );
            }
            SettingsChange::ToggleEditorSoftWrap => {
                self.editor.soft_wrap = !self.editor.soft_wrap;
            }
            SettingsChange::CycleEditorTabSize => {
                self.editor.tab_size = next_tab_size(self.editor.tab_size);
            }
        }
    }

    /// 把 `editor.tab_size` 映射成 [`BufferConfig`]——主工作区 / 嵌入式文档构造缓冲区前从这里取。
    /// `tab_size = 0` 时 `BufferConfig::default`保留默认值。
    pub(crate) fn buffer_config(&self) -> BufferConfig {
        let mut buffer_config = BufferConfig::default();
        if let Some(width) = NonZeroUsize::new(self.editor.tab_size as usize) {
            buffer_config.tab = TabConfig::new(width, width, true);
        }
        buffer_config
    }

    pub(crate) fn normalized(mut self) -> Self {
        if !matches!(self.general.theme.as_str(), THEME_ONE_DARK) {
            self.general.theme = THEME_ONE_DARK.to_string();
        }
        self.ui.font_size = self.ui.font_size.clamp(UI_FONT_MIN, UI_FONT_MAX);
        self.editor.font_size = self
            .editor
            .font_size
            .clamp(EDITOR_FONT_MIN, EDITOR_FONT_MAX);
        if !TAB_SIZES.contains(&self.editor.tab_size) {
            self.editor.tab_size = 4;
        }
        self
    }
}

fn stepped(current: u16, delta: i16, min: u16, max: u16) -> u16 {
    if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs()).clamp(min, max)
    } else {
        current.saturating_add(delta as u16).clamp(min, max)
    }
}

fn next_tab_size(current: u16) -> u16 {
    let index = TAB_SIZES.iter().position(|size| *size == current);
    match index {
        Some(index) => TAB_SIZES[(index + 1) % TAB_SIZES.len()],
        None => 4,
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
        Ok(config) => config.normalized(),
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
        config.general.theme = THEME_ONE_DARK.to_string();
        config.ui.font_size = 14;
        config.editor.soft_wrap = false;
        config.editor.font_size = 18;
        config.editor.tab_size = 2;
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
