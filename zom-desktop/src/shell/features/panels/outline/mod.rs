//! Outline —— L3 panel 组件。
//!
//! 当前渲染「占位中」灰字；LSP 接入后填充符号大纲。

use gpui::{Context, Div, FocusHandle, IntoElement};
use zom_command::PanelKind;

use crate::host_intent::KeyRequest;
use crate::shell::CommandTitleLookup;
use crate::shell::workbench::docks::{placeholder, render_focus_host};

const COMMAND: &str = PanelKind::Outline.toggle_command_id();

#[derive(Clone)]
pub(crate) struct OutlineRuntime {
    focus: FocusHandle,
}

impl OutlineRuntime {
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
