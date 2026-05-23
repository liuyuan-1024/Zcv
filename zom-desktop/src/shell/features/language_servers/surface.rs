//! 语言服务器 surface 的 L3 组件。
//!
//! 第一版只承载静态状态骨架；后续接入真实语言服务器 registry 时只改本模块
//! 和状态输入，不影响 SurfaceShell / 命令系统。

use std::rc::Rc;

use gpui::{Corner, Div, div, point, prelude::*, px};

use crate::shell::shared::theme::{color, radius, space, typography};
use crate::shell::surfaces::{
    SurfaceAnchor, SurfaceId, SurfaceInvokerPoint, SurfacePlacement, SurfaceRequest,
};

pub(crate) fn request() -> SurfaceRequest {
    SurfaceRequest {
        id: SurfaceId::LanguageServers,
        anchor: SurfaceAnchor::Invoker(super::INVOKER_ID.into()),
        placement: SurfacePlacement {
            invoker_point: SurfaceInvokerPoint::TopLeft,
            corner: Corner::BottomLeft,
            offset: point(px(0.0), -space::s8()),
            fallback_position: point(px(48.0), px(540.0)),
        },
        focus_on_open: None,
        render: Rc::new(|| render().into_any_element()),
    }
}

fn render() -> Div {
    div()
        .w(px(280.0))
        .p(space::s12())
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::g40())
        .bg(color::gray::g10())
        .child(
            div()
                .text_size(typography::ui())
                .text_color(color::gray::g95())
                .child(super::FEATURE_TITLE),
        )
        .child(
            div()
                .mt(space::s8())
                .text_size(typography::ui())
                .text_color(color::gray::g60())
                .child("当前文件暂无已连接的语言服务器"),
        )
        .child(
            div()
                .mt(space::s12())
                .p(space::s8())
                .rounded(radius::r4())
                .bg(color::gray::g20())
                .text_size(typography::ui())
                .text_color(color::gray::g90())
                .child("等待语言服务器接入"),
        )
}
