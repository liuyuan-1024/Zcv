//! 语言服务器 overlay 的 L3 组件。
//!
//! 第一版只承载静态状态骨架；后续接入真实语言服务器 registry 时只改本模块
//! 和状态输入，不影响 OverlayShell / 命令系统。

use gpui::{Div, div, prelude::*, px};

use crate::shell::shared::theme::{color, radius, space, typography};

pub(crate) fn render() -> Div {
    div()
        .w(px(280.0))
        .p(space::s12())
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::g40())
        .bg(color::gray::g10())
        .child(
            div()
                .text_size(typography::title())
                .text_color(color::gray::g95())
                .child("语言服务器"),
        )
        .child(
            div()
                .mt(space::s8())
                .text_size(typography::caption())
                .text_color(color::gray::g60())
                .child("当前文件暂无已连接的语言服务器"),
        )
        .child(
            div()
                .mt(space::s12())
                .p(space::s8())
                .rounded(radius::r4())
                .bg(color::gray::g20())
                .text_size(typography::body())
                .text_color(color::gray::g90())
                .child("等待语言服务器接入"),
        )
}
