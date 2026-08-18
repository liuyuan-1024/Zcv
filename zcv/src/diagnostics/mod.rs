//! DiagnosticsButton —— 底栏诊断状态按钮。
//!
//! 对标 Zed 的 `DiagnosticIndicator`。当前为占位，后续接入诊断计数。

use gpui::{Context, Render, Window, prelude::*};
use zcv_ui::Glyph;
use zcv_workspace::ItemHandle;
use zcv_workspace::{FocusOrHidePanel, StatusItemView};

pub(crate) struct DiagnosticsButton;

impl DiagnosticsButton {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl StatusItemView for DiagnosticsButton {
    fn set_active_pane_item(&mut self, _item: Option<&dyn ItemHandle>, _cx: &mut Context<Self>) {}
}

impl Render for DiagnosticsButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let action = FocusOrHidePanel::new("diagnostics");
        Glyph::icon_text("diagnostics-button", "icons/warning.svg", "0")
            .label("诊断")
            .shortcut(&action, cx)
            .on_click(move |_, window, cx| window.dispatch_action(Box::new(action.clone()), cx))
            .into_any_element()
    }
}
