//! OverlayShell —— 可交互悬浮层 portal（布局模型 7 / 手册 21）。
//!
//! 本模块只负责“把 active overlay 投影到 portal”。具体 overlay 的视觉组件
//! 留在各自模块内，避免 shell 入口累积业务 UI。

use gpui::{
    AnyElement, Context, Corner, Entity, Render, Subscription, Window, anchored, deferred, div,
    point, prelude::*, px,
};

use super::{
    ActiveOverlay, AnchorRegistry, OverlayAnchor, OverlayKind, OverlayManager, project_picker,
};
use crate::shell::theme::space;

pub(crate) struct OverlayShell {
    manager: Entity<OverlayManager>,
    anchors: Entity<AnchorRegistry>,
    _manager_observer: Subscription,
    _anchor_observer: Subscription,
}

impl OverlayShell {
    pub(crate) fn new(
        manager: Entity<OverlayManager>,
        anchors: Entity<AnchorRegistry>,
        cx: &mut Context<Self>,
    ) -> Self {
        let manager_observer = cx.observe(&manager, |_, _, cx| cx.notify());
        let anchor_observer = cx.observe(&anchors, |_, _, cx| cx.notify());
        Self {
            manager,
            anchors,
            _manager_observer: manager_observer,
            _anchor_observer: anchor_observer,
        }
    }
}

impl Render for OverlayShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self
            .manager
            .read_with(cx, |manager, _| manager.active().cloned());
        let Some(active) = active else {
            return div().absolute().top_0().left_0().size_full().invisible();
        };
        let anchor_bounds = self
            .anchors
            .read_with(cx, |anchors, _| anchors.resolve(active.anchor()));

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(deferred(render_active(active, anchor_bounds)).priority(30))
    }
}

fn render_active(
    active: ActiveOverlay,
    anchor_bounds: Option<gpui::Bounds<gpui::Pixels>>,
) -> impl IntoElement {
    anchored()
        .anchor(Corner::TopLeft)
        .position(anchor_position(active.anchor(), anchor_bounds))
        .offset(point(px(0.0), space::s8()))
        .snap_to_window_with_margin(space::s8())
        .child(render_kind(active.kind()))
}

fn render_kind(kind: OverlayKind) -> AnyElement {
    match kind {
        OverlayKind::ProjectPicker => project_picker::render().into_any_element(),
    }
}

fn anchor_position(
    anchor: &OverlayAnchor,
    anchor_bounds: Option<gpui::Bounds<gpui::Pixels>>,
) -> gpui::Point<gpui::Pixels> {
    if let Some(bounds) = anchor_bounds {
        return bounds.bottom_left();
    }

    match anchor {
        // anchor provider 首帧尚未 prepaint 或临时缺席时，退回 top bar leading
        // 区域的保守位置；下一帧 registry 更新后会自动重绘到真实锚点。
        OverlayAnchor::Element(_) => point(px(48.0), px(28.0)),
    }
}
