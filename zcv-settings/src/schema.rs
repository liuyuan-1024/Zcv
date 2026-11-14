//! 设置文件的内容 schema 与值校验。
//!
//! 只描述 JSON 层可表达的内容与解析规则；默认值合并与运行时表示由 `merge` 层负责。

use std::{
    collections::HashMap,
    fmt,
    marker::PhantomData,
    num::{NonZeroU32, NonZeroUsize},
};

use anyhow::{Context as _, Result, ensure};
use globset::Glob;
use serde::{Deserialize, Deserializer, Serialize, de::Visitor};
use zcv_theme::ThemeChoice;

/// 软换行模式的设置值。
///
/// - `none`：不换行，超长行靠水平滚动查看；
/// - `editor-width`：行宽超过编辑器文本区宽度时换行，窗口 resize 实时重排；
/// - `bounded`：在 `preferred_line_length`（列数 × em 宽）与编辑器宽度（取小者）处换行。
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SoftWrapMode {
    None,
    EditorWidth,
    Bounded,
}

/// 自动缩进策略。
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutoIndentMode {
    None,
    PreserveIndent,
    #[default]
    SyntaxAware,
}

/// 编辑器光标形状。
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    #[default]
    Bar,
    Block,
    Underline,
    Hollow,
}

/// 终端光标形状由设置解析层确定，消费方不再处理未知字符串。
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalCursorShape {
    Block,
    Underline,
    Bar,
    Hollow,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CursorAnimationSettingsContent {
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) enabled: Option<bool>,
}

/// 内置默认层由项目维护，缺少任一配置项都属于开发错误。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuiltinSettingsContent {
    pub(crate) theme: ThemeChoice,
    pub(crate) content_font_size: f32,
    pub(crate) ui_font_size: f32,
    pub(crate) content_line_height: f32,
    pub(crate) tab_size: NonZeroUsize,
    pub(crate) insert_spaces: bool,
    /// 按语言的内置默认缩进策略；键为语言展示名，字段缺省沿用全局值。
    pub(crate) languages: HashMap<String, LanguageOverrideContent>,
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
    pub(crate) remove_trailing_whitespace_on_save: bool,
    pub(crate) ensure_final_newline_on_save: bool,
    pub(crate) terminal_font_size: f32,
    pub(crate) terminal_line_height: f32,
    pub(crate) terminal_max_scroll_history_lines: usize,
    pub(crate) terminal_cursor_shape: TerminalCursorShape,
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
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum TerminalShellSetting {
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
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LanguageOverrideContent {
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub tab_size: Option<NonZeroUsize>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub insert_spaces: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub indent_guides: Option<IndentGuideSettingsContent>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub remove_trailing_whitespace_on_save: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub ensure_final_newline_on_save: Option<bool>,
}

/// 一门语言的 Tab / 缩进覆盖值。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LanguageOverride {
    pub tab_size: Option<NonZeroUsize>,
    pub insert_spaces: Option<bool>,
    pub(crate) indent_guides: Option<IndentGuideSettingsContent>,
    pub remove_trailing_whitespace_on_save: Option<bool>,
    pub ensure_final_newline_on_save: Option<bool>,
}

/// 按语言解析后的引导线策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndentGuideSettings {
    pub enabled: bool,
    pub line_width: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct IndentGuideSettingsContent {
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub enabled: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub line_width: Option<NonZeroU32>,
}

impl IndentGuideSettingsContent {
    pub(crate) fn apply(self, settings: &mut IndentGuideSettings) {
        if let Some(value) = self.enabled {
            settings.enabled = value;
        }
        if let Some(value) = self.line_width {
            settings.line_width = value.get();
        }
    }
}

/// 用户设置文件的完整覆盖内容；字段为 `None` 表示未在用户文件中填写。
///
/// 该类型是面板与手写 `settings.jsonc` 共用的可编辑表示：两者都先经过
/// `parse_user_settings` / `validate_user_settings`，再由 `merge` 生成运行时值。
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct UserSettingsContent {
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub theme: Option<ThemeChoice>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub content_font_size: Option<f32>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub ui_font_size: Option<f32>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub content_line_height: Option<f32>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub tab_size: Option<NonZeroUsize>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub insert_spaces: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub indent_guides: Option<IndentGuideSettingsContent>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub soft_wrap: Option<SoftWrapMode>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub minimum_contrast_for_highlights: Option<f32>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub cursor_shape: Option<CursorShape>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub cursor_blink: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub cursor_animation: Option<CursorAnimationSettingsContent>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub preferred_line_length: Option<usize>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub file_scan_exclusions: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub use_autoclose: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub use_auto_surround: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub auto_indent: Option<AutoIndentMode>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub extend_comment_on_newline: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub extend_list_on_newline: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub remove_trailing_whitespace_on_save: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub ensure_final_newline_on_save: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub terminal_font_size: Option<f32>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub terminal_line_height: Option<f32>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub terminal_max_scroll_history_lines: Option<usize>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub terminal_cursor_shape: Option<TerminalCursorShape>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub terminal_alternate_scroll: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub terminal_option_as_meta: Option<bool>,
    /// 缺失与显式 null 分别表示“不覆盖内置值”和“使用系统 shell”。
    #[serde(
        default,
        deserialize_with = "deserialize_terminal_shell",
        skip_serializing_if = "Option::is_none"
    )]
    pub terminal_shell: Option<TerminalShellSetting>,
    /// 按语言覆盖的 Tab / 缩进策略；键为语言展示名。
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub languages: Option<HashMap<String, LanguageOverrideContent>>,
}

/// 缺失字段由 `#[serde(default)]` 处理；字段一旦出现，显式 null 必须报错而不是被当作未填写。
fn deserialize_non_null_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct NonNullOption<T>(PhantomData<T>);

    impl<'de, T> Visitor<'de> for NonNullOption<T>
    where
        T: Deserialize<'de>,
    {
        type Value = Option<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("非 null 的配置值")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Err(E::custom("配置项不能为 null；删除该字段表示使用内置默认值"))
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Err(E::custom("配置项不能为 null；删除该字段表示使用内置默认值"))
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            T::deserialize(deserializer).map(Some)
        }
    }

    deserializer.deserialize_option(NonNullOption(PhantomData))
}

/// 缺失与显式 null 分别表示“不覆盖内置值”和“使用系统 shell”。
fn deserialize_terminal_shell<'de, D>(
    deserializer: D,
) -> Result<Option<TerminalShellSetting>, D::Error>
where
    D: Deserializer<'de>,
{
    struct TerminalShellVisitor;

    impl<'de> Visitor<'de> for TerminalShellVisitor {
        type Value = Option<TerminalShellSetting>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("null 或 shell 程序字符串")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(TerminalShellSetting::System))
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(TerminalShellSetting::System))
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            String::deserialize(deserializer)
                .map(|shell| Some(TerminalShellSetting::Program(shell)))
        }
    }

    deserializer.deserialize_option(TerminalShellVisitor)
}

pub(crate) fn parse_user_settings(content: &str) -> Result<UserSettingsContent> {
    if content.trim().is_empty() {
        return Ok(UserSettingsContent::default());
    }
    let settings: UserSettingsContent =
        serde_json_lenient::from_str(content).context("不是合法的 settings JSONC")?;
    validate_user_settings(&settings)?;
    Ok(settings)
}

/// 用户文件与面板写入共用同一套值校验；解析只负责 JSON 结构与类型。
pub(crate) fn validate_user_settings(settings: &UserSettingsContent) -> Result<()> {
    validate_positive_numbers(&[
        ("content_font_size", settings.content_font_size),
        ("ui_font_size", settings.ui_font_size),
        ("content_line_height", settings.content_line_height),
        ("terminal_font_size", settings.terminal_font_size),
        ("terminal_line_height", settings.terminal_line_height),
    ])?;
    if let Some(length) = settings.preferred_line_length {
        ensure!(length > 0, "preferred_line_length 必须大于 0");
    }
    if let Some(guides) = &settings.indent_guides {
        validate_indent_guide_width(guides.line_width, "indent_guides")?;
    }
    if let Some(languages) = &settings.languages {
        for (name, overrides) in languages {
            if let Some(guides) = overrides.indent_guides {
                validate_indent_guide_width(guides.line_width, name)?;
            }
        }
    }
    if let Some(TerminalShellSetting::Program(shell)) = &settings.terminal_shell {
        ensure!(!shell.is_empty(), "terminal_shell 不能是空字符串");
    }
    if let Some(exclusions) = &settings.file_scan_exclusions {
        validate_exclusions(exclusions)?;
    }
    if let Some(value) = settings.minimum_contrast_for_highlights {
        ensure!(
            valid_minimum_contrast(value),
            "高亮文字最低对比度必须在 0 到 106 之间"
        );
    }
    Ok(())
}

pub(crate) fn parse_builtin_settings(content: &str) -> Result<BuiltinSettingsContent> {
    let settings: BuiltinSettingsContent =
        serde_json_lenient::from_str(content).context("内置初始设置不是合法的 settings JSONC")?;
    validate_positive_numbers(&[
        ("content_font_size", Some(settings.content_font_size)),
        ("ui_font_size", Some(settings.ui_font_size)),
        ("content_line_height", Some(settings.content_line_height)),
        ("terminal_font_size", Some(settings.terminal_font_size)),
        ("terminal_line_height", Some(settings.terminal_line_height)),
    ])?;
    ensure!(
        settings.preferred_line_length > 0,
        "preferred_line_length 必须大于 0"
    );
    if let TerminalShellSetting::Program(shell) = &settings.terminal_shell {
        ensure!(!shell.is_empty(), "terminal_shell 不能是空字符串");
    }
    ensure!(
        settings.indent_guides.line_width.get() <= 10,
        "内置引导线宽度必须在 1 到 10 之间"
    );
    for (language, overrides) in &settings.languages {
        if let Some(guides) = overrides.indent_guides {
            validate_indent_guide_width(guides.line_width, language)?;
        }
    }
    ensure!(
        valid_minimum_contrast(settings.minimum_contrast_for_highlights),
        "内置高亮文字最低对比度必须在 0 到 106 之间"
    );
    validate_exclusions(&settings.file_scan_exclusions)?;
    Ok(settings)
}

fn validate_positive_numbers(values: &[(&str, Option<f32>)]) -> Result<()> {
    for (name, value) in values {
        if let Some(value) = value {
            ensure!(value.is_finite() && *value > 0.0, "{name} 必须大于 0");
        }
    }
    Ok(())
}

fn validate_indent_guide_width(value: Option<NonZeroU32>, name: &str) -> Result<()> {
    if let Some(value) = value {
        ensure!(value.get() <= 10, "{name} 的引导线宽度必须在 1 到 10 之间");
    }
    Ok(())
}

fn validate_exclusions(exclusions: &[String]) -> Result<()> {
    for exclusion in exclusions {
        Glob::new(exclusion).with_context(|| format!("无效的文件扫描排除规则：{exclusion}"))?;
    }
    Ok(())
}

pub(crate) fn valid_minimum_contrast(value: f32) -> bool {
    value.is_finite() && (0.0..=106.0).contains(&value)
}
