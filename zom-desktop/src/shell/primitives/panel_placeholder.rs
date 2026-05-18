//! Panel 占位渲染。
//!
//! 第一版骨架阶段：各 L3 panel 的内部尚未接入真实数据源，统一渲染
//! 「标题 + 占位中」灰字提示，让布局骨架立刻可视。
//!
//! 这是一个 L2 视觉原语（与具体 panel 解耦），各 panel 自己的 `view.rs`
//! 只描述「我是什么」「我占位时显示什么文本」。

use gpui::{Div, div, prelude::*};

use crate::shell::theme::{color, space, typography};

pub(crate) fn panel_placeholder(title: &'static str, hint: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .size_full()
        .gap(space::s8())
        .child(
            div()
                .text_size(typography::caption())
                .text_color(color::gray::g60())
                .child(title),
        )
        .child(
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
