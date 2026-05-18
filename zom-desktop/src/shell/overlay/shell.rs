//! OverlayShell —— 可交互悬浮层 portal（布局模型 7 / 手册 21）。
//!
//! 本模块只负责“把 active overlay 投影到 portal”。具体 overlay 的视觉组件
//! 留在各自模块内，避免 shell 入口累积业务 UI。

use gpui::{
    AnyElement, Context, Corner, Entity, Render, Subscription, Window, anchored, deferred, div,
    point, prelude::*, px,
};

use super::{ActiveOverlay, AnchorRegistry, OverlayAnchor, OverlayKind, OverlayManager};
use crate::shell::ActionRequest;
use crate::shell::features::{language_servers, project_picker};
use crate::shell::shared::theme::space;

pub(crate) struct OverlayShell {
    manager: Entity<OverlayManager>,
    anchors: Entity<AnchorRegistry>,
    open_local_project: ActionRequest,
    _manager_observer: Subscription,
    _anchor_observer: Subscription,
}

impl OverlayShell {
    pub(crate) fn new(
        manager: Entity<OverlayManager>,
        anchors: Entity<AnchorRegistry>,
        open_local_project: ActionRequest,
        cx: &mut Context<Self>,
    ) -> Self {
        let manager_observer = cx.observe(&manager, |_, _, cx| cx.notify());
        let anchor_observer = cx.observe(&anchors, |_, _, cx| cx.notify());
        Self {
            manager,
            anchors,
            open_local_project,
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

        div().absolute().top_0().left_0().size_full().child(
            deferred(render_active(
                active,
                anchor_bounds,
                self.open_local_project.clone(),
            ))
            .priority(30),
        )
    }
}

fn render_active(
    active: ActiveOverlay,
    anchor_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    open_local_project: ActionRequest,
) -> impl IntoElement {
    let kind = active.kind();
    anchored()
        .anchor(anchor_corner(kind))
        .position(anchor_position(kind, active.anchor(), anchor_bounds))
        .offset(anchor_offset(kind))
        .snap_to_window_with_margin(space::s8())
        .child(render_kind(kind, open_local_project))
}

fn render_kind(kind: OverlayKind, open_local_project: ActionRequest) -> AnyElement {
    match kind {
        OverlayKind::ProjectPicker => project_picker::render(open_local_project).into_any_element(),
        OverlayKind::LanguageServers => language_servers::render().into_any_element(),
    }
}

fn anchor_corner(kind: OverlayKind) -> Corner {
    match kind {
        OverlayKind::ProjectPicker => Corner::TopLeft,
        OverlayKind::LanguageServers => Corner::BottomLeft,
    }
}

fn anchor_offset(kind: OverlayKind) -> gpui::Point<gpui::Pixels> {
    match kind {
        OverlayKind::ProjectPicker => point(px(0.0), space::s8()),
        OverlayKind::LanguageServers => point(px(0.0), -space::s8()),
    }
}

fn anchor_position(
    kind: OverlayKind,
    anchor: &OverlayAnchor,
    anchor_bounds: Option<gpui::Bounds<gpui::Pixels>>,
) -> gpui::Point<gpui::Pixels> {
    if let Some(bounds) = anchor_bounds {
        return match kind {
            OverlayKind::ProjectPicker => bounds.bottom_left(),
            OverlayKind::LanguageServers => bounds.origin,
        };
    }

    match anchor {
        // anchor provider 首帧尚未 prepaint 或临时缺席时，退回对应 bar 区域的
        // 保守位置；下一帧 registry 更新后会自动重绘到真实锚点。
        OverlayAnchor::Element(_) => match kind {
            OverlayKind::ProjectPicker => point(px(48.0), px(28.0)),
            OverlayKind::LanguageServers => point(px(48.0), px(540.0)),
        },
    }
}
