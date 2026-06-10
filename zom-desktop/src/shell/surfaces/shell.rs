//! SurfaceShell —— 可交互浮面 portal（布局模型 7 / 手册 21）。

use gpui::{
    Context, Entity, MouseButton, Render, Subscription, Window, anchored, deferred, div, prelude::*,
};

use gpui::Corner;

use super::{ActiveSurface, SurfaceAnchor, SurfaceAnchorRegistry, SurfaceManager, WindowPosition};
use crate::theme::space;

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
            SurfaceAnchor::Invoker { id, .. } => {
                cx.read_global::<SurfaceAnchorRegistry, _>(|anchors, _| {
                    anchors.resolve_anchor(window, id)
                })
            }
            SurfaceAnchor::Window { .. } => None,
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
    let focus_on_click = request.focus_on_open.clone();
    let surface = div()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if let Some(focus) = &focus_on_click {
                window.focus(focus);
            }
            cx.stop_propagation();
        })
        .child((request.render)());

    match &request.anchor {
        SurfaceAnchor::Window {
            position: WindowPosition::Center,
        } => div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(surface)
            .into_any_element(),
        SurfaceAnchor::Invoker {
            attachment,
            fallback_position,
            ..
        } => {
            let position = anchor_bounds
                .map(|bounds| match attachment {
                    Corner::TopLeft => bounds.bottom_left(),
                    Corner::TopRight => bounds.bottom_right(),
                    Corner::BottomLeft => bounds.origin,
                    Corner::BottomRight => bounds.top_right(),
                })
                .unwrap_or(*fallback_position);
            anchored()
                .anchor(*attachment)
                .position(position)
                .snap_to_window_with_margin(space::s8())
                .child(surface)
                .into_any_element()
        }
    }
}
