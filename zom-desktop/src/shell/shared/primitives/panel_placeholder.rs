//! Panel 占位渲染。
//!
//! 第一版骨架阶段：各 L3 panel 的内部尚未接入真实数据源，统一在中央渲染
//! 一行灰字提示，让布局骨架立刻可视。标题归 dock header 渲染，panel 内部
//! 不再重复。

use gpui::{Div, div, prelude::*};

use crate::shell::shared::theme::{color, typography};

pub(crate) fn panel_placeholder(hint: &'static str) -> Div {
    div().flex().flex_col().size_full().child(
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(typography::body())
            .text_color(color::gray::g60())
            .child(hint),
    )
}
