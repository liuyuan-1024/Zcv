//! 项目选择器 surface 的 L3 组件。
//!
//! 这里负责项目选择器自己的浮面定义：内容、召唤入口、尺寸与偏移都跟功能放在一起。

use std::rc::Rc;

use gpui::{Corner, Div, div, point, prelude::*, px};

use crate::shell::ActionRequest;
use crate::shell::shared::theme::{color, radius, space, typography};
use crate::shell::surfaces::{
    SurfaceAnchor, SurfaceId, SurfaceInvokerPoint, SurfacePlacement, SurfaceRequest,
};

pub(crate) fn request(open_local_project: ActionRequest) -> SurfaceRequest {
    SurfaceRequest {
        id: SurfaceId::ProjectPicker,
        anchor: SurfaceAnchor::Invoker(super::INVOKER_ID.into()),
        placement: SurfacePlacement {
            invoker_point: SurfaceInvokerPoint::BottomLeft,
            corner: Corner::TopLeft,
            offset: point(px(0.0), space::s8()),
            fallback_position: point(px(48.0), px(28.0)),
        },
        render: Rc::new(move || render(open_local_project.clone()).into_any_element()),
    }
}

fn render(open_local_project: ActionRequest) -> Div {
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
                .child(super::FEATURE_TITLE),
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
