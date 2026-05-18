//! RightDock —— 右停靠区 L4 Region（布局模型 4.4 / 手册 20.1）。
//!
//! 与 LeftDock 视觉对称——只有分隔线方向相反；规则、状态、PanelHost 接入完全一致。

use gpui::{Div, div, prelude::*};

use crate::shell::model::DockState;
use crate::shell::panels::PanelHost;
use crate::shell::theme::{color, space, typography};

use crate::shell::primitives::{DockEdge, dock_frame};

pub(crate) fn render(state: &DockState, host: &PanelHost) -> Div {
    let body = match state.active_panel() {
        Some(id) => host.render(id),
        None => empty_body().into_any_element(),
    };

    dock_frame(DockEdge::Right)
        .w(state.size)
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
