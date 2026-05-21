//! Terminal —— L3 panel 组件。
//!
//! 第一版骨架：渲染「占位中」灰字。

use gpui::{Context, Div, FocusHandle, IntoElement};

use crate::shell::KeyRequest;
use crate::shell::workbench::docks::{placeholder, render_focus_host};

pub(crate) const PANEL_ICON: &str = "icons/bottom_bar/terminal.svg";
pub(crate) const PANEL_TITLE: &str = "终端";

#[derive(Clone)]
pub(crate) struct TerminalRuntime {
    focus: FocusHandle,
}

impl TerminalRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn render(&self, key_request: &KeyRequest) -> Div {
        render_focus_host(
            &self.focus,
            key_request,
            placeholder("终端占位中").into_any_element(),
        )
    }
}
