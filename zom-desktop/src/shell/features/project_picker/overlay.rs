//! 项目选择器 overlay 的 L3 组件。
//!
//! 这里只负责项目选择器自己的视觉结构；打开、关闭、锚点选择由 overlay
//! manager / shell 入口处理。

use gpui::{Div, div, prelude::*, px};

use crate::shell::ActionRequest;
use crate::shell::shared::theme::{color, radius, space, typography};

pub(crate) fn render(open_local_project: ActionRequest) -> Div {
    div()
        .w(px(320.0))
        .p(space::s12())
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::g40())
        .bg(color::gray::g10())
        .child(
            div()
                .text_size(typography::ui())
                .text_color(color::gray::g95())
                .child("切换项目"),
        )
        .child(
            div()
                .mt(space::s8())
                .text_size(typography::ui())
                .text_color(color::gray::g60())
                .child("最近项目与打开本地文件夹入口占位中"),
        )
        .child(
            div()
                .id("project-picker.open-local-project")
                .mt(space::s12())
                .p(space::s8())
                .rounded(radius::r4())
                .bg(color::gray::g20())
                .text_size(typography::ui())
                .text_color(color::gray::g90())
                .cursor_pointer()
                .on_click(move |_, window, cx| open_local_project(window, cx))
                .child("打开本地文件夹"),
        )
}
