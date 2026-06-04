//! 语言服务器 surface 的 L3 组件。
//!
//! 当前承载静态状态入口；接入真实语言服务器 registry 时只改本模块
//! 和状态输入，不影响 SurfaceShell / 命令系统。

use std::rc::Rc;

use gpui::{Context, Corner, Div, Entity, FocusHandle, Window, div, point, prelude::*, px};

use crate::shell::shared::theme::{color, radius, space, typography};
use crate::shell::surfaces::{
    SurfaceAnchor, SurfaceId, SurfaceInvokerPoint, SurfaceManager, SurfacePlacement, SurfaceRequest,
};

#[derive(Clone)]
pub(crate) struct LanguageServersRuntime {
    focus: FocusHandle,
}

impl LanguageServersRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn install_listeners<T: 'static>(
        &self,
        surfaces: Entity<SurfaceManager>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let focus = self.focus.clone();
        cx.on_blur(&focus, window, move |_, _, cx| {
            surfaces.update(cx, |surfaces, cx| {
                if surfaces.is_active(SurfaceId::LanguageServers) {
                    surfaces.dismiss(cx);
                }
            });
            cx.notify();
        })
        .detach();
    }
}

pub(crate) fn request(runtime: LanguageServersRuntime) -> SurfaceRequest {
    let focus = runtime.focus.clone();
    SurfaceRequest {
        id: SurfaceId::LanguageServers,
        anchor: SurfaceAnchor::Invoker(super::INVOKER_ID.into()),
        placement: SurfacePlacement {
            invoker_point: SurfaceInvokerPoint::TopLeft,
            corner: Corner::BottomLeft,
            offset: point(px(0.0), -space::s6()),
            fallback_position: point(px(48.0), px(540.0)),
        },
        focus_on_open: Some(focus),
        render: Rc::new(move || render(&runtime.focus).into_any_element()),
    }
}

fn render(focus: &FocusHandle) -> Div {
    div()
        .w(px(280.0))
        .p(space::s6())
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::s05())
        .bg(color::gray::s03())
        .track_focus(focus)
        .tab_index(0)
        .child(
            div()
                .text_size(typography::ui())
                .text_color(color::gray::s08())
                .child("当前文件暂无已连接的语言服务器"),
        )
        .child(
            div()
                .rounded(radius::r4())
                .bg(color::gray::s04())
                .text_size(typography::ui())
                .text_color(color::gray::s09())
                .child("等待语言服务器接入"),
        )
}
