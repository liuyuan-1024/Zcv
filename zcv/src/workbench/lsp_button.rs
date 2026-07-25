//! LspButton —— 底栏语言服务器状态按钮。
//!
//! 对标 Zed 的 `LspButton`。当前为占位，后续接入 LSP 状态。

use gpui::{Context, Entity, Render, Window, prelude::*};

use crate::editor::editor::Editor;
use crate::ui::glyph::Glyph;
use crate::workbench::dock::ToggleLanguageServer;
use crate::workbench::status_bar::StatusItemView;

pub(crate) struct LspButton;

impl LspButton {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl StatusItemView for LspButton {
    fn set_active_editor(&mut self, _editor: Option<&Entity<Editor>>, _cx: &mut Context<Self>) {}
}

impl Render for LspButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        Glyph::icon("lsp-button", "icons/status/language_server.svg")
            .label("语言服务器")
            .shortcut_by_name("dock::ToggleLanguageServer", cx)
            .on_click(|window, cx| window.dispatch_action(Box::new(ToggleLanguageServer), cx))
            .into_any_element()
    }
}
