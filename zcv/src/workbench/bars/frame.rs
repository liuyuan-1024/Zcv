//! Shell bar 通用框架与内部小件。
//!
//! `TopBar` 与 `BottomBar` 共用同一套 shell bar 视觉规则（布局模型 4.1）：
//! 高度、内边距、背景、字号统一；只有分隔线方向相反。本文件汇总 bar 共用的几样：
//!
//! - `bar_frame`：bar 外壳（背景 / 上下边框 / 内边距 / 默认字号色）。
//! - `BarRegionAlign` / `align_bar_region`：两端对齐（leading / trailing）。
//! - `bar_divider`：槽内分组用的 1px 短线。

use gpui::{Div, div, prelude::*};

use crate::theme::{color, space};

// ═══ 公开渲染函数 ═════════════════════════════════════════════════════

/// 渲染共享的 shell bar 外壳。
/// 三槽（leading / center / trailing）由调用方分别填充。
pub(crate) fn bar_frame(edge: BarEdge) -> Div {
    let frame = div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .px(space::S8)
        .py(space::S6)
        .gap(space::S8)
        .bg(color::current().gray.s[2])
        .text_color(color::current().gray.s[8]);

    match edge {
        BarEdge::Top => frame.border_b_1().border_color(color::current().gray.s[4]),
        BarEdge::Bottom => frame.border_t_1().border_color(color::current().gray.s[4]),
    }
}

/// 把内部内容包成统一的对齐区段，使两端布局稳定。
pub(crate) fn align_bar_region(inner: Div, align: BarRegionAlign) -> Div {
    let wrapper = div().flex_1().flex().items_center().gap(space::S8);
    match align {
        BarRegionAlign::Leading => wrapper.justify_start().child(inner),
        BarRegionAlign::Trailing => wrapper.justify_end().child(inner),
    }
}

/// 槽内分组用的 1px 竖线，高度自适应容器。
pub(crate) fn bar_divider() -> Div {
    div()
        .w(gpui::px(1.0))
        .h_full()
        .bg(color::current().gray.s[4])
}

// ═══ 内部类型 ═════════════════════════════════════════════════════════

#[derive(Clone, Copy)]
pub(crate) enum BarEdge {
    Top,
    Bottom,
}

#[derive(Clone, Copy)]
pub(crate) enum BarRegionAlign {
    Leading,
    Trailing,
}
