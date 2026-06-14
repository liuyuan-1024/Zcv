//! SurfaceShell —— 可交互浮面 portal（布局模型 7 / 手册 21）。
//!
//! 统一处理所有 surface 的字体链、按键路由和点击外部自动关闭。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Context, Entity, MouseButton, Render, Subscription, Window, anchored, deferred, div, prelude::*,
};

use gpui::Corner;

use super::{ActiveSurface, SurfaceAnchor, SurfaceAnchorRegistry, SurfaceManager, WindowPosition};
use crate::host_intent::KeyRequest;
use crate::shell::normalized_chord;
use crate::shell::{FocusRequestTarget, focus_request};
use crate::theme::{color, space, typography};

pub(crate) struct SurfaceShell {
    manager: Entity<SurfaceManager>,
    _manager_observer: Subscription,
    key_request: Rc<RefCell<Option<KeyRequest>>>,
}

impl SurfaceShell {
    pub(crate) fn new(manager: Entity<SurfaceManager>, cx: &mut Context<Self>) -> Self {
        let manager_observer = cx.observe(&manager, |_, _, cx| cx.notify());
        Self {
            manager,
            _manager_observer: manager_observer,
            key_request: Rc::new(RefCell::new(None)),
        }
    }

    pub(crate) fn set_key_request(&self, key_request: KeyRequest) {
        *self.key_request.borrow_mut() = Some(key_request);
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

        let key_request = Rc::clone(&self.key_request);
        let dismiss_manager = self.manager.clone();
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if let Some(focus) = dismiss_manager.update(cx, |m, cx| m.dismiss(cx)) {
                    window.focus(&focus);
                    window.refresh();
                }
            })
            .child(deferred(render_active(active, anchor_bounds, key_request)).priority(30))
    }
}

fn render_active(
    active: ActiveSurface,
    anchor_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    key_request: Rc<RefCell<Option<KeyRequest>>>,
) -> impl IntoElement {
    let request = active.request().clone();
    let focus_on_open = request.focus_on_open.clone();
    let focus_request = focus_request();
    let surface = div()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .font(typography::ui_font())
        .text_color(color::current().gray.s09)
        .on_key_down({
            let key_request = Rc::clone(&key_request);
            move |event, window, cx| {
                if let Some(key_request) = key_request.borrow().clone() {
                    if key_request(normalized_chord(&event.keystroke), window, cx) {
                        cx.stop_propagation();
                    }
                }
            }
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if let Some(focus) = &focus_on_open {
                focus_request(FocusRequestTarget::Handle(focus.clone()), window, cx);
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
