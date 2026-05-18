//! 项目选择器 overlay 的 L3 组件。
//!
//! 这里只负责项目选择器自己的视觉结构；打开、关闭、锚点选择由 overlay
//! manager / shell 入口处理。

use gpui::{Div, div, prelude::*, px};

use crate::shell::theme::{color, radius, space, typography};

pub(crate) fn render() -> Div {
    div()
        .w(px(320.0))
        .p(space::s12())
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::g40())
        .bg(color::gray::g10())
        .child(
            div()
                .text_size(typography::title())
                .text_color(color::gray::g95())
                .child("切换项目"),
        )
        .child(
            div()
                .mt(space::s8())
                .text_size(typography::caption())
                .text_color(color::gray::g60())
                .child("最近项目与打开本地文件夹入口占位中"),
        )
        .child(
            div()
                .mt(space::s12())
                .p(space::s8())
                .rounded(radius::r4())
                .bg(color::gray::g20())
                .text_size(typography::body())
                .text_color(color::gray::g90())
                .child("打开本地文件夹"),
        )
}
