//! 视觉 token：色彩、间距、排版、语法高亮。
//! 此文件是 `zcv-theme` crate 的公共入口。
//!
//! [`ThemeChoice`] 是主题配置入口：`System` 或注册表中的主题 id。
//! 主题数据（语义色 + 语法高亮）由 `theme_data` 注册表统一持有，新增主题只需添加 TOML 文件并在注册表登记，无需改动本模块逻辑。

pub mod color;
pub mod fixed;
mod icon_theme;
pub mod scale;
pub mod syntax;
mod theme_data;
pub mod typography;

use gpui::{App, Window};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use theme_data::{ThemeData, theme_by_id, themes};

pub use icon_theme::FileIcons;

/// 主题配置：跟随系统外观，或显式指定注册表中的主题（id 取自主题文件名）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Named(&'static str),
}

impl ThemeChoice {
    /// 设置与面板可选的完整主题列表：跟随系统在前，其后是注册表中的内置主题。
    pub fn available() -> impl Iterator<Item = Self> {
        std::iter::once(Self::System).chain(themes().iter().map(|theme| Self::Named(theme.id)))
    }

    /// 写回设置文件时使用的稳定名称。
    pub fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Named(id) => id,
        }
    }

    /// 从设置字符串解析；未知 id 是配置错误。
    pub fn from_config(s: &str) -> Result<Self, String> {
        match s {
            "system" => Ok(Self::System),
            _ => theme_by_id(s)
                .map(|theme| Self::Named(theme.id))
                .ok_or_else(|| format!("未知主题：{s}")),
        }
    }

    /// 解析为具体主题：`System` 按窗口外观选择匹配的主题，无窗口时默认深色。
    pub(crate) fn effective(self, window: Option<&Window>) -> &'static ThemeData {
        match self {
            Self::Named(id) => theme_by_id(id).expect("具名主题必须来自注册表"),
            Self::System => window
                .map(|w| w.appearance())
                .and_then(|appearance| themes().iter().find(|theme| theme.appearance == appearance))
                .unwrap_or_else(first_theme),
        }
    }

    pub fn apply(self, cx: &mut App, window: Option<&Window>) {
        let theme = self.effective(window);
        color::set_theme(theme, cx);
        syntax::set_theme(theme, cx);
    }
}

impl<'de> Deserialize<'de> for ThemeChoice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        Self::from_config(&name).map_err(D::Error::custom)
    }
}

impl Serialize for ThemeChoice {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.id())
    }
}

/// 无窗口且注册表为空时的兜底（内置深色主题）。
pub(crate) fn first_theme() -> &'static ThemeData {
    themes()
        .first()
        .expect("主题注册表不应为空（至少包含内置主题）")
}

#[cfg(test)]
#[path = "test/theme_tests.rs"]
mod tests;
