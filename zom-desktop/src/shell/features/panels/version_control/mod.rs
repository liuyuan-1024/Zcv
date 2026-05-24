//! VersionControl —— L3 panel 组件。
//!
//! 第一版骨架：渲染「占位中」灰字。

use gpui::{Context, Div, FocusHandle, IntoElement};
use zom_command::commands::version_control;

use crate::shell::workbench::docks::{placeholder, render_focus_host};
use crate::shell::{CommandTitleLookup, KeyRequest};

const COMMAND: &str = version_control::TOGGLE_PANEL;

#[derive(Clone)]
pub(crate) struct VersionControlRuntime {
    focus: FocusHandle,
}

impl VersionControlRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn render(&self, key_request: &KeyRequest, titles: &CommandTitleLookup) -> Div {
        let title = titles(COMMAND).unwrap_or_else(|| COMMAND.to_string());
        render_focus_host(
            &self.focus,
            key_request,
            placeholder(format!("{title}占位中")).into_any_element(),
        )
    }
}
