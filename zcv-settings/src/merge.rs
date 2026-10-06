//! 用户设置的默认值合并与运行时表示。
//!
//! 默认层的唯一数据源是内置 `initial_user_settings.json`。

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::LazyLock;

use super::INITIAL_USER_SETTINGS;
use super::schema::{
    AutoIndentMode, BuiltinSettingsContent, CursorShape, IndentGuideSettings, LanguageOverride,
    SoftWrapMode, TabConfig, UserSettingsContent, parse_builtin_settings,
};

#[derive(Clone, Debug, PartialEq)]
pub struct UserSettings {
    /// 主题配置 id；由主题模块解析为运行时主题。
    pub theme: String,
    /// 文档内容字号（像素）。编辑器与预览共用。
    pub content_font_size: f32,
    /// UI 字号（像素）。
    pub ui_font_size: f32,
    /// 文档内容行高（相对字号的倍数）。
    pub content_line_height: f32,
    /// Tab 展示宽度与缩进输入策略。
    pub tab: TabConfig,
    /// 按语言覆盖的 Tab / 缩进策略；键为语言展示名。
    pub languages: HashMap<String, LanguageOverride>,
    pub indent_guides: IndentGuideSettings,
    pub soft_wrap: SoftWrapMode,
    /// 高亮背景上的文字最低 APCA 对比度；0 关闭修正。
    pub minimum_contrast_for_highlights: f32,
    /// 编辑器光标形状。
    pub cursor_shape: CursorShape,
    /// 聚焦时编辑器光标是否闪烁。
    pub cursor_blink: bool,
    /// 是否在光标移动时播放动画。
    pub cursor_animation_enabled: bool,
    /// 软换行的目标行宽（列数）；仅在 `soft_wrap = "bounded"` 时生效。
    pub preferred_line_length: usize,
    /// 项目树扫描时完全排除的 glob 名单。
    pub file_scan_exclusions: Vec<String>,
    /// 键入配对起始字符时是否自动补全闭合符。
    pub use_autoclose: bool,
    /// 选中文本时键入配对起始字符是否用该对包裹选区。
    pub use_auto_surround: bool,
    /// 自动缩进策略；语言的 `indents.scm` 在语法感知模式下生效。
    pub auto_indent: AutoIndentMode,
    /// 换行时是否续写行注释与文档注释。
    pub extend_comment_on_newline: bool,
    /// 换行时是否续写 Markdown 列表。
    pub extend_list_on_newline: bool,
    /// 终端字体大小（像素）。
    pub terminal_font_size: f32,
    /// 终端行高（相对字号的倍数）。
    pub terminal_line_height: f32,
    /// 终端滚动回看上限行数。
    pub terminal_max_scroll_history_lines: usize,
    /// 终端光标形状："block" | "underline" | "bar"。
    pub terminal_cursor_shape: String,
    /// 备用屏幕下滚轮是否转发为方向键。
    pub terminal_alternate_scroll: bool,
    /// Option 键是否作为 Meta 键使用。
    pub terminal_option_as_meta: bool,
    /// 终端 shell 程序；缺省时使用系统默认 shell。
    pub terminal_shell: Option<String>,
}

/// 内置配置缺项或非法时立即失败，用户配置只负责逐字段覆盖。
fn default_content() -> &'static BuiltinSettingsContent {
    static DEFAULTS: LazyLock<BuiltinSettingsContent> = LazyLock::new(|| {
        parse_builtin_settings(&INITIAL_USER_SETTINGS).expect("内置初始设置必须完整且合法")
    });
    &DEFAULTS
}

impl Default for TabConfig {
    fn default() -> Self {
        let defaults = default_content();
        Self {
            tab_size: defaults.tab_size,
            insert_spaces: defaults.insert_spaces,
        }
    }
}

impl Default for IndentGuideSettings {
    fn default() -> Self {
        let defaults = default_content();
        Self {
            enabled: defaults.indent_guides.enabled,
            line_width: defaults.indent_guides.line_width.get(),
        }
    }
}

impl Default for UserSettings {
    fn default() -> Self {
        Self::merge(UserSettingsContent::default())
    }
}

impl UserSettings {
    /// 解析某语言的 Tab / 缩进策略：全局默认为底，按语言覆盖逐字段替换。
    ///
    /// `language_name` 为 `None`（未识别语言）时只返回全局默认。
    pub fn tab_for_language(&self, language_name: Option<&str>) -> TabConfig {
        let mut tab = self.tab;
        if let Some(over) = language_name.and_then(|name| self.languages.get(name)) {
            if let Some(value) = over.tab_size {
                tab.tab_size = value;
            }
            if let Some(value) = over.insert_spaces {
                tab.insert_spaces = value;
            }
        }
        tab
    }

    /// 引导线设置按语言逐字段覆盖全局值。
    pub fn indent_guides_for_language(&self, language_name: Option<&str>) -> IndentGuideSettings {
        let mut settings = self.indent_guides;
        if let Some(overrides) = language_name.and_then(|name| self.languages.get(name))
            && let Some(content) = overrides.indent_guides
        {
            content.apply(&mut settings);
        }
        settings
    }

    /// 将用户配置合并到内置默认层：用户显式配置的字段覆盖默认，未配置字段由内置初始设置补齐。
    pub(crate) fn merge(content: UserSettingsContent) -> Self {
        let defaults = default_content();
        Self {
            theme: content.theme.unwrap_or_else(|| defaults.theme.clone()),
            content_font_size: content
                .content_font_size
                .unwrap_or(defaults.content_font_size),
            ui_font_size: content.ui_font_size.unwrap_or(defaults.ui_font_size),
            content_line_height: content
                .content_line_height
                .unwrap_or(defaults.content_line_height),
            tab: TabConfig {
                tab_size: content
                    .tab_size
                    .and_then(NonZeroUsize::new)
                    .unwrap_or(defaults.tab_size),
                insert_spaces: content.insert_spaces.unwrap_or(defaults.insert_spaces),
            },
            languages: content
                .languages
                .unwrap_or_default()
                .into_iter()
                .map(|(language, content)| {
                    (
                        language,
                        LanguageOverride {
                            tab_size: content.tab_size.and_then(NonZeroUsize::new),
                            insert_spaces: content.insert_spaces,
                            indent_guides: content.indent_guides,
                        },
                    )
                })
                .collect(),
            indent_guides: {
                let mut settings = IndentGuideSettings {
                    enabled: defaults.indent_guides.enabled,
                    line_width: defaults.indent_guides.line_width.get(),
                };
                if let Some(content) = content.indent_guides {
                    content.apply(&mut settings);
                }
                settings
            },
            soft_wrap: content.soft_wrap.unwrap_or(defaults.soft_wrap),
            minimum_contrast_for_highlights: content
                .minimum_contrast_for_highlights
                .unwrap_or(defaults.minimum_contrast_for_highlights),
            cursor_shape: content.cursor_shape.unwrap_or(defaults.cursor_shape),
            cursor_blink: content.cursor_blink.unwrap_or(defaults.cursor_blink),
            cursor_animation_enabled: content
                .cursor_animation
                .and_then(|animation| animation.enabled)
                .unwrap_or(defaults.cursor_animation.enabled),
            preferred_line_length: content
                .preferred_line_length
                .unwrap_or(defaults.preferred_line_length),
            file_scan_exclusions: content
                .file_scan_exclusions
                .unwrap_or_else(|| defaults.file_scan_exclusions.clone()),
            use_autoclose: content.use_autoclose.unwrap_or(defaults.use_autoclose),
            use_auto_surround: content
                .use_auto_surround
                .unwrap_or(defaults.use_auto_surround),
            auto_indent: content.auto_indent.unwrap_or(defaults.auto_indent),
            extend_comment_on_newline: content
                .extend_comment_on_newline
                .unwrap_or(defaults.extend_comment_on_newline),
            extend_list_on_newline: content
                .extend_list_on_newline
                .unwrap_or(defaults.extend_list_on_newline),
            terminal_font_size: content
                .terminal_font_size
                .unwrap_or(defaults.terminal_font_size),
            terminal_line_height: content
                .terminal_line_height
                .unwrap_or(defaults.terminal_line_height),
            terminal_max_scroll_history_lines: content
                .terminal_max_scroll_history_lines
                .unwrap_or(defaults.terminal_max_scroll_history_lines),
            terminal_cursor_shape: content
                .terminal_cursor_shape
                .unwrap_or_else(|| defaults.terminal_cursor_shape.clone()),
            terminal_alternate_scroll: content
                .terminal_alternate_scroll
                .unwrap_or(defaults.terminal_alternate_scroll),
            terminal_option_as_meta: content
                .terminal_option_as_meta
                .unwrap_or(defaults.terminal_option_as_meta),
            terminal_shell: content
                .terminal_shell
                .unwrap_or_else(|| defaults.terminal_shell.clone())
                .into_runtime(),
        }
    }
}
