//! 语言解析后的编辑器设置。
//!
//! 对齐 Zed `LanguageSettings`：设置按 buffer/language 解析，消费方读取 Buffer 的解析结果，而不是在 Editor/DisplayMap 各处直接拼全局设置。
//! 全局默认来自 `zcv-settings`，语言级覆盖以语言展示名为键。

use std::sync::Arc;

use gpui::App;
pub use zcv_settings::{AutoIndentMode, IndentGuideSettings};
use zcv_settings::{SettingsStore, TabConfig, UserSettings};

/// 一门语言解析后的编辑器设置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageSettings {
    /// Tab 展示宽度与缩进输入策略。
    pub tab: TabConfig,
    pub indent_guides: IndentGuideSettings,
    pub auto_indent: AutoIndentMode,
    pub extend_comment_on_newline: bool,
    pub extend_list_on_newline: bool,
}

impl Default for LanguageSettings {
    fn default() -> Self {
        let settings = UserSettings::default();
        Self {
            tab: settings.tab,
            indent_guides: settings.indent_guides,
            auto_indent: settings.auto_indent,
            extend_comment_on_newline: settings.extend_comment_on_newline,
            extend_list_on_newline: settings.extend_list_on_newline,
        }
    }
}

impl LanguageSettings {
    /// 按语言解析设置；`SettingsStore` 未注册（如单元测试）时回退内置默认。
    pub fn resolve(language_name: Option<&str>, cx: &App) -> Arc<Self> {
        Arc::new(
            SettingsStore::try_get(cx).map_or_else(Self::default, |settings| Self {
                tab: settings.tab_for_language(language_name),
                indent_guides: settings.indent_guides_for_language(language_name),
                auto_indent: settings.auto_indent,
                extend_comment_on_newline: settings.extend_comment_on_newline,
                extend_list_on_newline: settings.extend_list_on_newline,
            }),
        )
    }
}
