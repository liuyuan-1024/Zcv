//! 语言解析后的编辑器设置。
//!
//! 对齐 Zed `LanguageSettings`：设置按 buffer/language 解析，消费方读取 Buffer 的解析结果，而不是在 Editor/DisplayMap 各处直接拼全局设置。
//! 全局默认来自 `zcv-settings`，语言级覆盖以语言展示名为键。

use std::sync::Arc;

use gpui::App;
use zcv_settings::{SettingsStore, TabConfig};

/// 一门语言解析后的编辑器设置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageSettings {
    /// Tab 展示宽度与缩进输入策略。
    pub tab: TabConfig,
}

impl LanguageSettings {
    /// 按语言解析设置；`SettingsStore` 未注册（如单元测试）时回退内置默认。
    pub fn resolve(language_name: Option<&str>, cx: &App) -> Arc<Self> {
        let tab = SettingsStore::try_get(cx).map_or_else(TabConfig::default, |settings| {
            settings.tab_for_language(language_name)
        });
        Arc::new(Self { tab })
    }
}

impl Default for LanguageSettings {
    fn default() -> Self {
        Self {
            tab: TabConfig::default(),
        }
    }
}
