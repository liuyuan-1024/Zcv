//! WorkbenchFrame —— 窗口顶层装配（布局模型 4 / 手册 二十一 z-index 分层）。
//!
//! ```text
//! ┌────────────────────────────────────┐
//! │ TopBar                             │
//! ├───────┬────────────────────┬───────┤
//! │ Left  │    EditorGrid      │ Right │
//! │ Dock  ├────────────────────┤ Dock  │
//! │       │    BottomDock      │       │
//! ├───────┴────────────────────┴───────┤
//! │ BottomBar                          │
//! └────────────────────────────────────┘
//! ```
//!
//! 渲染层次（手册 21.2）：
//!   [10] WorkbenchFrame
//!   [20] OverlayShell
//!   [30] BubbleShell
//! 后两层骨架阶段为空 portal，不参与 layout。

use gpui::{Div, Window, div, prelude::*};

use crate::shell::layout::WorkbenchState;
use crate::shell::panel_host::PanelHost;
use crate::shell::theme::{color, radius};

use super::{
    bottom_bar, bubble_layer, center_column, left_dock, overlay_layer, right_dock, top_bar,
};

pub(crate) fn render(state: &WorkbenchState, host: &PanelHost, window: &Window) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .rounded(radius::window())
        .border_1()
        .border_color(color::gray::g40())
        .bg(color::gray::g05())
        .text_color(color::gray::g90())
        .child(top_bar::render(window))
        .child(body(state, host))
        .child(bottom_bar::render(state))
        .child(overlay_layer::render())
        .child(bubble_layer::render())
}

fn body(state: &WorkbenchState, host: &PanelHost) -> Div {
    let mut row = div().flex_1().flex().flex_row().w_full().overflow_hidden();

    if state.left_dock.is_visible() {
        row = row.child(left_dock::render(&state.left_dock, host));
    }
    row = row.child(center_column::render(&state.bottom_dock, host));
    if state.right_dock.is_visible() {
        row = row.child(right_dock::render(&state.right_dock, host));
    }

    row
}
