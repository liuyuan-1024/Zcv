//! 设置文件的内容 schema 与字段级容错解析。
//!
//! 只描述 JSON 层可表达的内容与解析规则；默认值合并与运行时表示由 `merge` 层负责。

use std::collections::HashMap;
use std::num::NonZeroUsize;

use anyhow::{Context as _, Result};
use serde::Deserialize;

/// 软换行模式的设置值。
///
/// - `none`：不换行，超长行靠水平滚动查看；
/// - `editor-width`：行宽超过编辑器文本区宽度时换行，窗口 resize 实时重排；
/// - `bounded`：在 `preferred_line_length`（列数 × em 宽）与编辑器宽度（取小者）处换行。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SoftWrapMode {
    None,
    EditorWidth,
    Bounded,
}

/// Tab 展示宽度与缩进输入策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabConfig {
    /// 制表符的视觉列宽，必须大于 0。
    pub tab_width: NonZeroUsize,
    /// 自动缩进的宽度，必须大于 0。
    pub indent_width: NonZeroUsize,
    /// 缩进时是否使用空格替代真实的 '\t'。
    pub insert_spaces: bool,
}

impl TabConfig {
    pub fn tab_width(self) -> usize {
        self.tab_width.get()
    }

    pub fn indent_width(self) -> usize {
        self.indent_width.get()
    }
}

impl Default for TabConfig {
    fn default() -> Self {
        Self {
            tab_width: NonZeroUsize::new(4).expect("默认 tab 宽度必须大于 0"),
            indent_width: NonZeroUsize::new(4).expect("默认缩进宽度必须大于 0"),
            insert_spaces: true,
        }
    }
}

/// 按语言覆盖的 Tab / 缩进策略；字段为 `None` 时沿用全局默认。
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct LanguageTabOverrideContent {
    #[serde(deserialize_with = "fallible")]
    pub(crate) tab_width: Option<usize>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) indent_width: Option<usize>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) insert_spaces: Option<bool>,
}

/// 一门语言的 Tab / 缩进覆盖值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabOverride {
    pub tab_width: Option<NonZeroUsize>,
    pub indent_width: Option<NonZeroUsize>,
    pub insert_spaces: Option<bool>,
}

/// 字段级容错：该字段值非法时解析为「未配置」（`None`），由 merge 层用内置默认补齐，不影响其他字段。
/// JSON 语法错误仍整体失败。
///
/// 先解析成 `Value` 再转换：serde_json_lenient 对 enum 字段的非法值走 `peek_error` 路径且不消费 token，直接 `T::deserialize(...).ok()`会让后续字段错位；
/// `Value` 解析总是消费完整 token。
fn fallible<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json_lenient::Value::deserialize(deserializer)?;
    Ok(serde_json_lenient::from_value(value).ok())
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct UserSettingsContent {
    #[serde(deserialize_with = "fallible")]
    pub(crate) theme: Option<String>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) content_font_size: Option<f32>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) ui_font_size: Option<f32>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) content_line_height: Option<f32>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) tab_width: Option<usize>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) indent_width: Option<usize>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) insert_spaces: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) soft_wrap: Option<SoftWrapMode>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) preferred_line_length: Option<usize>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) file_scan_exclusions: Option<Vec<String>>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) use_autoclose: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) use_auto_surround: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) terminal_font_size: Option<f32>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) terminal_line_height: Option<f32>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) terminal_max_scroll_history_lines: Option<usize>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) terminal_cursor_shape: Option<String>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) terminal_alternate_scroll: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) terminal_option_as_meta: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) terminal_shell: Option<String>,
    /// 按语言覆盖的 Tab / 缩进策略；键为语言展示名。
    #[serde(deserialize_with = "fallible")]
    pub(crate) languages: Option<HashMap<String, LanguageTabOverrideContent>>,
}

pub(crate) fn parse_user_settings(content: &str) -> Result<UserSettingsContent> {
    if content.trim().is_empty() {
        return Ok(UserSettingsContent::default());
    }
    serde_json_lenient::from_str(content).context("不是合法的 settings JSON")
}
