//! DiagnosticsButton —— 底栏诊断状态按钮。
//!
//! 对标 Zed 的 `DiagnosticIndicator`。当前为占位，后续接入诊断计数。

use gpui::{Context, Entity, Render, Window, prelude::*};

use crate::workspace::StatusItemView;
use crate::workspace::ToggleDiagnostics;
use zcv_editor::Editor;
use zcv_ui::Glyph;

pub(crate) struct DiagnosticsButton;

impl DiagnosticsButton {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl StatusItemView for DiagnosticsButton {
    fn set_active_editor(&mut self, _editor: Option<&Entity<Editor>>, _cx: &mut Context<Self>) {}
}

impl Render for DiagnosticsButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        Glyph::icon_text("diagnostics-button", "icons/diagnostics.svg", "0")
            .label("诊断")
            .shortcut(&ToggleDiagnostics, cx)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleDiagnostics), cx))
            .into_any_element()
    }
}
