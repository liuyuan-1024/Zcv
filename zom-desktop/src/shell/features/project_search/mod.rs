//! ProjectSearch —— L3 panel 组件。
//!
//! 第一版骨架：渲染「占位中」灰字；P3 接入 zom-engine 搜索能力时再补 UI。

use gpui::{Context, Div, FocusHandle, IntoElement};

use crate::shell::KeyRequest;

use super::panel;

pub(crate) const PANEL_ICON: &str = "icons/bottom_bar/project_search.svg";

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

    pub(crate) fn render(&self, key_request: &KeyRequest) -> Div {
        panel::render_focus_host(
            &self.focus,
            key_request,
            panel::placeholder("项目搜索占位中").into_any_element(),
        )
    }
}
