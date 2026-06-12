//! Shell bar 通用框架与内部小件。
//!
//! `TopBar` 与 `BottomBar` 共用同一套 shell bar 视觉规则（布局模型 4.1）：
//! 高度、内边距、背景、字号统一；只有分隔线方向相反。本文件汇总 bar 共
//! 用的几样：
//!
//! - `bar_frame`：bar 外壳（背景 / 上下边框 / 内边距 / 默认字号色）。
//! - `BarRegionAlign` / `align_bar_region`：两端对齐（leading / trailing）。
//! - `bar_divider`：槽内分组用的 1px 短线。

use gpui::{Div, div, prelude::*};

use crate::theme::{color, space, typography};

#[derive(Clone, Copy)]
pub(crate) enum BarEdge {
    Top,
    Bottom,
}

/// 渲染共享的 shell bar 外壳。
/// 三槽（leading / center / trailing）由调用方分别填充。
pub(crate) fn bar_frame(edge: BarEdge) -> Div {
    let frame = div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .px(space::s6())
        .py(space::s4())
        .gap(space::s8())
        .bg(color::current().gray.s03)
        .text_color(color::current().gray.s09)
        .text_size(typography::ui());

    match edge {
        BarEdge::Top => frame.border_b_1().border_color(color::current().gray.s05),
        BarEdge::Bottom => frame.border_t_1().border_color(color::current().gray.s05),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum BarRegionAlign {
    Leading,
    Trailing,
}

/// 把内部内容包成统一的对齐区段，使两端布局稳定。
pub(crate) fn align_bar_region(inner: Div, align: BarRegionAlign) -> Div {
    let wrapper = div().flex_1().flex().items_center().gap(space::s8());
    match align {
        BarRegionAlign::Leading => wrapper.justify_start().child(inner),
        BarRegionAlign::Trailing => wrapper.justify_end().child(inner),
    }
}

/// 槽内分组用的 1px 短线（约一行字号高度）。属于「纯视觉标记」，
/// 高度可显式给出（见 `桌面端布局模型.md` 3.x 的豁免）。
pub(crate) fn bar_divider() -> Div {
    div()
        .w(gpui::px(1.0))
        .h(space::s16())
        .bg(color::current().gray.s05)
}
