//! SurfaceShell —— 可交互浮面 portal（布局模型 7 / 手册 21）。

use gpui::{Context, Entity, Render, Subscription, Window, anchored, deferred, div, prelude::*};

use super::{
    ActiveSurface, SurfaceAnchor, SurfaceAnchorRegistry, SurfaceInvokerPoint, SurfaceManager,
    SurfaceRequest,
};
use crate::shell::shared::theme::space;

pub(crate) struct SurfaceShell {
    manager: Entity<SurfaceManager>,
    _manager_observer: Subscription,
}

impl SurfaceShell {
    pub(crate) fn new(manager: Entity<SurfaceManager>, cx: &mut Context<Self>) -> Self {
        let manager_observer = cx.observe(&manager, |_, _, cx| cx.notify());
        Self {
            manager,
            _manager_observer: manager_observer,
        }
    }
}

impl Render for SurfaceShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self
            .manager
            .read_with(cx, |manager, _| manager.active().cloned());
        let Some(active) = active else {
            return div().absolute().top_0().left_0().size_full().invisible();
        };
        let anchor_bounds = match &active.request().anchor {
            SurfaceAnchor::Invoker(id) => {
                cx.read_global::<SurfaceAnchorRegistry, _>(|anchors, _| {
                    anchors.resolve_anchor(window, id)
                })
            }
        };

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(deferred(render_active(active, anchor_bounds)).priority(30))
    }
}

fn render_active(
    active: ActiveSurface,
    anchor_bounds: Option<gpui::Bounds<gpui::Pixels>>,
) -> impl IntoElement {
    let request = active.request().clone();
    let placement = request.placement.clone();
    anchored()
        .anchor(placement.corner)
        .position(anchor_position(&request, anchor_bounds))
        .offset(placement.offset)
        .snap_to_window_with_margin(space::s8())
        .child((request.render)())
}

fn anchor_position(
    request: &SurfaceRequest,
    anchor_bounds: Option<gpui::Bounds<gpui::Pixels>>,
) -> gpui::Point<gpui::Pixels> {
    if let Some(bounds) = anchor_bounds {
        return match request.placement.invoker_point {
            SurfaceInvokerPoint::TopLeft => bounds.origin,
            SurfaceInvokerPoint::BottomLeft => bounds.bottom_left(),
        };
    }

    match &request.anchor {
        // anchor provider 首帧尚未 prepaint 或临时缺席时，退回对应 bar 区域
        // 的保守位置；下一帧 registry 更新后会自动重绘到真实入口。
        SurfaceAnchor::Invoker(_) => request.placement.fallback_position,
    }
}
