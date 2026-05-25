//! Dock 通用框架。
//!
//! `LeftDock` / `RightDock` / `BottomDock` 共用同一套外壳视觉：背景、内边距、
//! 与相邻区域之间的分隔线。它们的差异只在分隔线的方向。

use gpui::{Div, div, prelude::*};

use crate::shell::shared::theme::{color, space};

#[derive(Clone, Copy)]
pub(crate) enum DockEdge {
    /// 左停靠区：分隔线在右侧。
    Left,
    /// 右停靠区：分隔线在左侧。
    Right,
    /// 中间列底部停靠区：分隔线在顶部。
    Bottom,
}

/// 渲染共享的 Dock 外壳。内部由调用方填充 panel 标题 + panel body。
pub(crate) fn dock_frame(edge: DockEdge) -> Div {
    let frame = div()
        .relative()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .h_full()
        .bg(color::gray::s02())
        .text_color(color::gray::s09())
        .p(space::s4());

    match edge {
        DockEdge::Left => frame.border_r_1().border_color(color::gray::s05()),
        DockEdge::Right => frame.border_l_1().border_color(color::gray::s05()),
        DockEdge::Bottom => frame.border_t_1().border_color(color::gray::s05()).w_full(),
    }
}
