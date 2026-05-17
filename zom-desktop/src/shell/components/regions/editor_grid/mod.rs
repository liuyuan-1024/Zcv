//! EditorGrid —— 主编辑区 L4 Region（布局模型 4.6 / 手册 19 / 20.9）。
//!
//! 不走 Panel 模型。第一版骨架渲染单 group 占位提示；后续按 19 章
//! 展开 split tree、tab group 与 EditorView。

use gpui::{Div, div, prelude::*};

use crate::shell::theme::{color, space, typography};

pub(crate) fn render() -> Div {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .bg(color::gray::g05())
        .text_color(color::gray::g75())
        .p(space::s16())
        .child(
            div()
                .text_size(typography::caption())
                .text_color(color::gray::g60())
                .child("EditorGrid"),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(typography::body())
                .text_color(color::gray::g60())
                .child("编辑区占位中"),
        )
}
