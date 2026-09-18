//! Markdown 文件预览。
//!
//! 此 crate 只负责把 Markdown 文档投影为原生预览 Item；
//! 标签生命周期、预览与源码切换由 `zcv-workspace::Pane` 统一管理。

mod document;
mod provider;
mod view;

use gpui::App;
use provider::MarkdownPreviewProvider;

/// 注册 Markdown Preview Provider。可重复调用。
pub fn init(cx: &mut App) {
    // Provider 只做语言识别，没有 Project 上下文；在注册时创建并持有自己的注册表。
    let language_registry = std::sync::Arc::new(zcv_language::LanguageRegistry::new());
    zcv_workspace::register(MarkdownPreviewProvider { language_registry }, cx);
}
