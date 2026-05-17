//! BottomDock —— 中间列底部停靠区 L4 Region（布局模型 4.5 / 手册 20.9）。
//!
//! 只位于 `EditorGrid` 下方，水平边界与 EditorGrid 对齐；不侵入左右 Dock。

use gpui::{Div, div, prelude::*};

use crate::shell::layout::DockState;
use crate::shell::panel_host::PanelHost;
use crate::shell::theme::{color, space, typography};

use crate::shell::components::primitives::{DockEdge, dock_frame};

pub(crate) fn render(state: &DockState, host: &PanelHost) -> Div {
    let body = match state.active_panel() {
        Some(id) => host.render(id),
        None => empty_body().into_any_element(),
    };

    dock_frame(DockEdge::Bottom)
        .h(state.size)
        .gap(space::s8())
        .child(header(state))
        .child(div().flex_1().child(body))
}

fn header(state: &DockState) -> Div {
    let title = state
        .active_panel()
        .map(|panel| panel.title())
        .unwrap_or("");

    div()
        .text_size(typography::caption())
        .text_color(color::gray::g60())
        .child(title)
}

fn empty_body() -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size_full()
        .text_size(typography::caption())
        .text_color(color::gray::g60())
        .child("无活动面板")
}
