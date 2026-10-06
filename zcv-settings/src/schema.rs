//! 设置文件的内容 schema 与字段级容错解析。
//!
//! 只描述 JSON 层可表达的内容与解析规则；默认值合并与运行时表示由 `merge` 层负责。

use std::collections::HashMap;
use std::num::{NonZeroU32, NonZeroUsize};

use anyhow::{Context as _, Result, ensure};
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

/// 自动缩进策略。
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutoIndentMode {
    None,
    PreserveIndent,
    #[default]
    SyntaxAware,
}

/// 编辑器光标形状。
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    #[default]
    Bar,
    Block,
    Underline,
    Hollow,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct CursorAnimationSettingsContent {
    #[serde(deserialize_with = "fallible")]
    pub(crate) enabled: Option<bool>,
}

/// 内置默认层由项目维护，缺少任一配置项都属于开发错误。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuiltinSettingsContent {
    pub(crate) theme: String,
    pub(crate) content_font_size: f32,
    pub(crate) ui_font_size: f32,
    pub(crate) content_line_height: f32,
    pub(crate) tab_size: NonZeroUsize,
    pub(crate) insert_spaces: bool,
    pub(crate) indent_guides: BuiltinIndentGuideSettings,
    pub(crate) soft_wrap: SoftWrapMode,
    pub(crate) minimum_contrast_for_highlights: f32,
    pub(crate) cursor_shape: CursorShape,
    pub(crate) cursor_blink: bool,
    pub(crate) cursor_animation: BuiltinCursorAnimationSettings,
    pub(crate) preferred_line_length: usize,
    pub(crate) file_scan_exclusions: Vec<String>,
    pub(crate) use_autoclose: bool,
    pub(crate) use_auto_surround: bool,
    pub(crate) auto_indent: AutoIndentMode,
    pub(crate) extend_comment_on_newline: bool,
    pub(crate) extend_list_on_newline: bool,
    pub(crate) terminal_font_size: f32,
    pub(crate) terminal_line_height: f32,
    pub(crate) terminal_max_scroll_history_lines: usize,
    pub(crate) terminal_cursor_shape: String,
    pub(crate) terminal_alternate_scroll: bool,
    pub(crate) terminal_option_as_meta: bool,
    pub(crate) terminal_shell: TerminalShellSetting,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuiltinIndentGuideSettings {
    pub(crate) enabled: bool,
    pub(crate) line_width: NonZeroU32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuiltinCursorAnimationSettings {
    pub(crate) enabled: bool,
}

/// `null` 明确表示使用系统默认 shell；缺少用户配置键则表示不覆盖内置值。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub(crate) enum TerminalShellSetting {
    System,
    Program(String),
}

impl TerminalShellSetting {
    pub(crate) fn into_runtime(self) -> Option<String> {
        match self {
            Self::System => None,
            Self::Program(shell) => Some(shell),
        }
    }
}

/// Tab 展示宽度与缩进输入策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabConfig {
    /// 制表符的视觉列宽与一级缩进宽度，必须大于 0。
    pub tab_size: NonZeroUsize,
    /// 缩进时是否使用空格替代真实的 '\t'。
    pub insert_spaces: bool,
}

impl TabConfig {
    pub fn tab_size(self) -> usize {
        self.tab_size.get()
    }
}

/// 按语言覆盖的 Tab / 缩进策略；字段为 `None` 时沿用全局默认。
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct LanguageOverrideContent {
    #[serde(deserialize_with = "fallible")]
    pub(crate) tab_size: Option<usize>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) insert_spaces: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) indent_guides: Option<IndentGuideSettingsContent>,
}

/// 一门语言的 Tab / 缩进覆盖值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageOverride {
    pub tab_size: Option<NonZeroUsize>,
    pub insert_spaces: Option<bool>,
    pub(crate) indent_guides: Option<IndentGuideSettingsContent>,
}

/// 按语言解析后的引导线策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndentGuideSettings {
    pub enabled: bool,
    pub line_width: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct IndentGuideSettingsContent {
    #[serde(deserialize_with = "fallible")]
    pub(crate) enabled: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) line_width: Option<u32>,
}

impl IndentGuideSettingsContent {
    pub(crate) fn apply(self, settings: &mut IndentGuideSettings) {
        if let Some(value) = self.enabled {
            settings.enabled = value;
        }
        if let Some(value) = self.line_width {
            settings.line_width = value.clamp(1, 10);
        }
    }
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
    pub(crate) tab_size: Option<usize>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) insert_spaces: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) indent_guides: Option<IndentGuideSettingsContent>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) soft_wrap: Option<SoftWrapMode>,
    pub(crate) minimum_contrast_for_highlights: Option<f32>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) cursor_shape: Option<CursorShape>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) cursor_blink: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) cursor_animation: Option<CursorAnimationSettingsContent>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) preferred_line_length: Option<usize>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) file_scan_exclusions: Option<Vec<String>>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) use_autoclose: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) use_auto_surround: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) auto_indent: Option<AutoIndentMode>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) extend_comment_on_newline: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    pub(crate) extend_list_on_newline: Option<bool>,
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
    pub(crate) terminal_shell: Option<TerminalShellSetting>,
    /// 按语言覆盖的 Tab / 缩进策略；键为语言展示名。
    #[serde(deserialize_with = "fallible")]
    pub(crate) languages: Option<HashMap<String, LanguageOverrideContent>>,
}

pub(crate) fn parse_user_settings(content: &str) -> Result<UserSettingsContent> {
    if content.trim().is_empty() {
        return Ok(UserSettingsContent::default());
    }
    let settings: UserSettingsContent =
        serde_json_lenient::from_str(content).context("不是合法的 settings JSONC")?;
    if let Some(value) = settings.minimum_contrast_for_highlights {
        ensure!(
            valid_minimum_contrast(value),
            "高亮文字最低对比度必须在 0 到 106 之间"
        );
    }
    Ok(settings)
}

pub(crate) fn parse_builtin_settings(content: &str) -> Result<BuiltinSettingsContent> {
    let settings: BuiltinSettingsContent =
        serde_json_lenient::from_str(content).context("内置初始设置不是合法的 settings JSONC")?;
    ensure!(
        settings.indent_guides.line_width.get() <= 10,
        "内置引导线宽度必须在 1 到 10 之间"
    );
    ensure!(
        valid_minimum_contrast(settings.minimum_contrast_for_highlights),
        "内置高亮文字最低对比度必须在 0 到 106 之间"
    );
    Ok(settings)
}

pub(crate) fn valid_minimum_contrast(value: f32) -> bool {
    value.is_finite() && (0.0..=106.0).contains(&value)
}
