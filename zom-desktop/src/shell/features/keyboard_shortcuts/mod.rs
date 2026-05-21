//! KeyboardShortcuts —— L3 panel 组件。
//!
//! 第一版骨架：渲染「占位中」灰字。

use gpui::{Context, Div, FocusHandle, IntoElement};

use crate::shell::KeyRequest;

use super::panel;

pub(crate) const PANEL_ICON: &str = "icons/bottom_bar/keyboard_shortcuts.svg";

#[derive(Clone)]
pub(crate) struct KeyboardShortcutsRuntime {
    focus: FocusHandle,
}

impl KeyboardShortcutsRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn render(&self, key_request: &KeyRequest) -> Div {
        panel::render_focus_host(
            &self.focus,
            key_request,
            panel::placeholder("快捷键占位中").into_any_element(),
        )
    }
}
