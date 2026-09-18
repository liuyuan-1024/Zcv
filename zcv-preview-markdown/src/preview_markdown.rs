//! Markdown 文件预览。
//!
//! 此 crate 只负责把 Markdown 文档投影为原生预览 Item；
//! 标签生命周期、预览与源码切换由 `zcv-workspace::Pane` 统一管理。

mod document;
mod provider;
mod view;

use std::sync::Arc;

use gpui::App;
use zcv_language::LanguageRegistry;

use provider::MarkdownPreviewProvider;

/// 注册 Markdown Preview Provider。可重复调用。
///
/// 语言注册表由应用装配层创建并注入；Provider 只做文件识别，不持有独立的注册表。
pub fn init(language_registry: Arc<LanguageRegistry>, cx: &mut App) {
    zcv_workspace::register(MarkdownPreviewProvider { language_registry }, cx);
}
