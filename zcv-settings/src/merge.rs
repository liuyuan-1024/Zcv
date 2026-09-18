//! 用户设置的默认值合并与运行时表示。
//!
//! 默认层的唯一数据源是内置 `initial_user_settings.json`。

use std::collections::HashMap;
use std::num::NonZeroUsize;

use super::INITIAL_USER_SETTINGS;
use super::schema::{SoftWrapMode, TabConfig, TabOverride, UserSettingsContent};

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
    pub languages: HashMap<String, TabOverride>,
    pub soft_wrap: SoftWrapMode,
    /// 软换行的目标行宽（列数）；仅在 `soft_wrap = "bounded"` 时生效。
    pub preferred_line_length: usize,
    /// 项目树扫描时完全排除的 glob 名单。
    pub file_scan_exclusions: Vec<String>,
    /// 键入配对起始字符时是否自动补全闭合符。
    pub use_autoclose: bool,
    /// 选中文本时键入配对起始字符是否用该对包裹选区。
    pub use_auto_surround: bool,
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

/// 解析内置初始设置作为默认层，保证默认值只有一个数据源。
fn default_content() -> UserSettingsContent {
    serde_json_lenient::from_str(&INITIAL_USER_SETTINGS).expect("内置初始设置应合法")
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
            if let Some(value) = over.tab_width {
                tab.tab_width = value;
            }
            if let Some(value) = over.indent_width {
                tab.indent_width = value;
            }
            if let Some(value) = over.insert_spaces {
                tab.insert_spaces = value;
            }
        }
        tab
    }

    /// 将用户配置合并到内置默认层：用户显式配置的字段覆盖默认，未配置的字段（`None`）回退到内置初始设置。
    pub(crate) fn merge(content: UserSettingsContent) -> Self {
        let defaults = default_content();
        let default_tab = TabConfig::default();
        // 默认值唯一数据源是内置 initial_user_settings.json。
        Self {
            theme: content.theme.or(defaults.theme).expect("内置默认应存在"),
            content_font_size: content
                .content_font_size
                .or(defaults.content_font_size)
                .expect("内置默认应存在"),
            ui_font_size: content
                .ui_font_size
                .or(defaults.ui_font_size)
                .expect("内置默认应存在"),
            content_line_height: content
                .content_line_height
                .or(defaults.content_line_height)
                .expect("内置默认应存在"),
            tab: TabConfig {
                tab_width: content
                    .tab_width
                    .or(defaults.tab_width)
                    .and_then(NonZeroUsize::new)
                    .unwrap_or(default_tab.tab_width),
                indent_width: content
                    .indent_width
                    .or(defaults.indent_width)
                    .and_then(NonZeroUsize::new)
                    .unwrap_or(default_tab.indent_width),
                insert_spaces: content
                    .insert_spaces
                    .or(defaults.insert_spaces)
                    .unwrap_or(default_tab.insert_spaces),
            },
            languages: content
                .languages
                .unwrap_or_default()
                .into_iter()
                .map(|(language, content)| {
                    (
                        language,
                        TabOverride {
                            tab_width: content.tab_width.and_then(NonZeroUsize::new),
                            indent_width: content.indent_width.and_then(NonZeroUsize::new),
                            insert_spaces: content.insert_spaces,
                        },
                    )
                })
                .collect(),
            soft_wrap: content
                .soft_wrap
                .or(defaults.soft_wrap)
                .expect("内置默认应存在"),
            preferred_line_length: content
                .preferred_line_length
                .or(defaults.preferred_line_length)
                .expect("内置默认应存在"),
            file_scan_exclusions: content
                .file_scan_exclusions
                .or(defaults.file_scan_exclusions)
                .expect("内置默认应存在"),
            use_autoclose: content
                .use_autoclose
                .or(defaults.use_autoclose)
                .expect("内置默认应存在"),
            use_auto_surround: content
                .use_auto_surround
                .or(defaults.use_auto_surround)
                .expect("内置默认应存在"),
            terminal_font_size: content
                .terminal_font_size
                .or(defaults.terminal_font_size)
                .expect("内置默认应存在"),
            terminal_line_height: content
                .terminal_line_height
                .or(defaults.terminal_line_height)
                .expect("内置默认应存在"),
            terminal_max_scroll_history_lines: content
                .terminal_max_scroll_history_lines
                .or(defaults.terminal_max_scroll_history_lines)
                .expect("内置默认应存在"),
            terminal_cursor_shape: content
                .terminal_cursor_shape
                .or(defaults.terminal_cursor_shape)
                .expect("内置默认应存在"),
            terminal_alternate_scroll: content
                .terminal_alternate_scroll
                .or(defaults.terminal_alternate_scroll)
                .expect("内置默认应存在"),
            terminal_option_as_meta: content
                .terminal_option_as_meta
                .or(defaults.terminal_option_as_meta)
                .expect("内置默认应存在"),
            terminal_shell: content.terminal_shell.or(defaults.terminal_shell),
        }
    }
}
