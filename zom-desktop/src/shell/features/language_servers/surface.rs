//! 语言服务器 surface 的 L3 组件。
//!
//! 当前承载静态状态入口；接入真实语言服务器 registry 时只改本模块
//! 和状态输入，不影响 SurfaceShell / 命令系统。

use std::rc::Rc;

use gpui::{Context, Corner, FocusHandle, div, point, prelude::*, px};

use crate::shell::surfaces::{SurfaceAnchor, SurfaceRequest};
use crate::theme::{color, radius, space};
use crate::ui_id::SurfaceId;

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
}

pub(crate) fn request(runtime: LanguageServersRuntime) -> SurfaceRequest {
    let focus = runtime.focus.clone();
    SurfaceRequest {
        id: SurfaceId::LanguageServers,
        anchor: SurfaceAnchor::Invoker {
            id: super::INVOKER_ID.into(),
            attachment: Corner::BottomLeft,
            fallback_position: point(px(48.0), px(540.0)),
        },
        focus_on_open: Some(focus.clone()),
        render: Rc::new(move || {
            div()
                .w(px(280.0))
                .p(space::s6())
                .rounded(radius::r4())
                .border_1()
                .border_color(color::current().gray.s05)
                .bg(color::current().gray.s03)
                .track_focus(&focus)
                .tab_index(0)
                .child(
                    div()
                        .text_color(color::current().gray.s08)
                        .child("当前文件暂无已连接的语言服务器"),
                )
                .child(
                    div()
                        .rounded(radius::r4())
                        .bg(color::current().gray.s04)
                        .text_color(color::current().gray.s09)
                        .child("等待语言服务器接入"),
                )
                .into_any_element()
        }),
    }
}
