//! SurfaceShell —— 浮面渲染实体。
//!
//! 全屏遮罩 + `Anchored` 锚定定位，点击遮罩自动关闭。

use gpui::{Context, MouseButton, Render, Window, anchored, deferred, div, point, prelude::*, px};

use crate::theme::{color, space};

use super::manager::SurfaceManager;
use super::{SurfaceAnchor, SurfaceRequest};

/// 浮面渲染组件。
///
/// 无状态：每帧从 `SurfaceManager` Global 读取当前状态渲染。
pub(crate) struct SurfaceShell;

impl SurfaceShell {
    pub fn new() -> Self {
        Self
    }
}

impl Render for SurfaceShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = cx.global::<SurfaceManager>().active().cloned();

        let Some(active) = active else {
            return div();
        };

        let request = active.request().clone();

        // 解析锚点位置
        let anchor_point = match &request.anchor {
            SurfaceAnchor::Position { point, .. } => Some(*point),
            SurfaceAnchor::Center => None,
        };

        // 全屏遮罩 + 浮面内容
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            // 点击遮罩关闭 surface
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                let focus = cx.update_global::<SurfaceManager, _>(|m, _| m.dismiss());
                if let Some(focus) = focus {
                    window.focus(&focus);
                }
                window.refresh();
            })
            .child(deferred(render_content(request, anchor_point)).with_priority(30))
    }
}

/// 渲染浮面内容，根据锚点类型选择定位方式。
fn render_content(
    request: SurfaceRequest,
    anchor_point: Option<gpui::Point<gpui::Pixels>>,
) -> impl IntoElement {
    let surface = div()
        .text_color(color::current().gray.s[8])
        .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
            cx.stop_propagation();
        })
        .child((request.render)());

    match &request.anchor {
        SurfaceAnchor::Center => div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(surface)
            .into_any_element(),

        SurfaceAnchor::Position { corner, .. } => {
            let pos = anchor_point.unwrap_or(point(px(0.0), px(0.0)));
            anchored()
                .anchor(*corner)
                .position(pos)
                .snap_to_window_with_margin(space::S8)
                .child(surface)
                .into_any_element()
        }
    }
}
