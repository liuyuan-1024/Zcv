//! ProjectSearch —— L3 panel 组件。
//!
//! 第一版骨架：渲染「占位中」灰字；P3 接入 zom-engine 搜索能力时再补 UI。

use gpui::{Context, Div, FocusHandle, IntoElement};
use zom_command::commands::project_search;

use crate::shell::workbench::docks::{placeholder, render_focus_host};
use crate::shell::{CommandTitleLookup, KeyRequest};

pub(crate) const PANEL_ICON: &str = "icons/bottom_bar/project_search.svg";

const COMMAND: &str = project_search::TOGGLE_PANEL;

#[derive(Clone)]
pub(crate) struct ProjectSearchRuntime {
    focus: FocusHandle,
}

impl ProjectSearchRuntime {
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
