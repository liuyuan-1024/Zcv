//! LspButton —— 底栏语言服务器状态按钮。
//!
//! 对标 Zed 的 `LspButton`。当前为占位，后续接入 LSP 状态。

use gpui::{Context, Render, Window, prelude::*};
use zcv_ui::Glyph;
use zcv_workspace::ItemHandle;
use zcv_workspace::{StatusItemView, ToggleLanguageServer};

pub(crate) struct LspButton;

impl LspButton {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl StatusItemView for LspButton {
    fn set_active_pane_item(&mut self, _item: Option<&dyn ItemHandle>, _cx: &mut Context<Self>) {}
}

impl Render for LspButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        Glyph::icon("lsp-button", "icons/bolt_outlined.svg")
            .label("语言服务器")
            .shortcut(&ToggleLanguageServer, cx)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleLanguageServer), cx))
            .into_any_element()
    }
}
