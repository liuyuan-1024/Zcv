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
    zcv_workspace::register(MarkdownPreviewProvider, cx);
}
