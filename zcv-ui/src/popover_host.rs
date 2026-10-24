//! 顶栏浮层的开合、互斥、焦点归还与展示外壳。

use std::{
    cell::RefCell,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};

use gpui::{
    Anchor, App, FocusHandle, MouseButton, MouseDownEvent, Pixels, WeakFocusHandle, Window,
    anchored, deferred, div, point, prelude::*,
};
use zcv_theme::{color, scale};

/// 浮层内容视图调用的关闭回调。
pub type PopoverDismissHandler = Box<dyn Fn(&mut Window, &mut App)>;

type OutsideClickHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// 浮层身份：同身份重复打开 = 关闭（toggle 语义）。
#[derive(Clone, Copy, PartialEq, Eq)]
struct OverlayId(usize);

impl OverlayId {
    fn next() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// 同一顶栏的所有浮层共享一个活动会话；其他窗口拥有独立实例。
#[derive(Clone, Default)]
pub struct PopoverLayer {
    active: Rc<RefCell<Option<ActiveOverlay>>>,
}

struct ActiveOverlay {
    id: OverlayId,
    focus_scope: FocusHandle,
    previous_focus: Option<WeakFocusHandle>,
}

impl ActiveOverlay {
    fn restore_focus(self, window: &mut Window, cx: &mut App) {
        if self.focus_scope.contains_focused(window, cx)
            && let Some(previous) = self.previous_focus.and_then(|focus| focus.upgrade())
        {
            window.focus(&previous, cx);
        }
    }
}

impl PopoverLayer {
    fn activate(
        &self,
        id: OverlayId,
        focus_scope: FocusHandle,
        previous_focus: Option<WeakFocusHandle>,
    ) {
        let mut active = self.active.borrow_mut();
        let previous_focus = active
            .take()
            .map_or(previous_focus, |current| current.previous_focus);
        *active = Some(ActiveOverlay {
            id,
            focus_scope,
            previous_focus,
        });
    }

    fn is_open(&self, id: OverlayId) -> bool {
        self.active
            .borrow()
            .as_ref()
            .is_some_and(|active| active.id == id)
    }

    fn toggle(
        &self,
        id: OverlayId,
        focus_scope: FocusHandle,
        previous_focus: Option<WeakFocusHandle>,
    ) -> (bool, Option<ActiveOverlay>) {
        let mut active = self.active.borrow_mut();
        match active.take() {
            Some(current) if current.id == id => (false, Some(current)),
            Some(current) => {
                *active = Some(ActiveOverlay {
                    id,
                    focus_scope,
                    previous_focus: current.previous_focus,
                });
                (true, None)
            }
            None => {
                *active = Some(ActiveOverlay {
                    id,
                    focus_scope,
                    previous_focus,
                });
                (true, None)
            }
        }
    }

    fn dismiss(&self, id: OverlayId) -> Option<ActiveOverlay> {
        let mut active = self.active.borrow_mut();
        if active.as_ref().is_some_and(|current| current.id == id) {
            active.take()
        } else {
            None
        }
    }
}

/// 顶栏浮层与焦点会话外壳；调用方负责内容和目标焦点。
pub struct PopoverHost {
    id: OverlayId,
    focus: FocusHandle,
    layer: PopoverLayer,
}

impl PopoverHost {
    pub fn new(focus: FocusHandle, layer: PopoverLayer) -> Self {
        Self {
            id: OverlayId::next(),
            focus,
            layer,
        }
    }

    pub fn is_open(&self) -> bool {
        self.layer.is_open(self.id)
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 聚焦常驻入口；重复聚焦不覆盖本次交互的归还目标。
    pub fn activate(&self, window: &mut Window, cx: &App) {
        let previous = window.focused(cx).map(|focus| focus.downgrade());
        self.layer.activate(self.id, self.focus.clone(), previous);
        window.refresh();
    }

    /// 焦点已转移到其他目标时，仅结束本入口在顶栏浮层中的登记。
    pub fn clear(&self) {
        self.layer.dismiss(self.id);
    }

    /// 供内容视图的关闭回调使用。
    pub fn on_dismiss_handler(&self) -> PopoverDismissHandler {
        let id = self.id;
        let layer = self.layer.clone();
        Box::new(move |window, app| {
            if let Some(active) = layer.dismiss(id) {
                active.restore_focus(window, app);
            }
            window.refresh();
        })
    }

    pub fn dismiss(&self, window: &mut Window, cx: &mut App) {
        if let Some(active) = self.layer.dismiss(self.id) {
            active.restore_focus(window, cx);
            window.refresh();
        }
    }

    /// 打开时聚焦目标；关闭时把焦点还给发起位置。
    pub fn toggle(
        &mut self,
        focus_scope: FocusHandle,
        focus_target: FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) {
        let previous_focus = window.focused(cx).map(|focus| focus.downgrade());
        let (opened, closed) = self.layer.toggle(self.id, focus_scope, previous_focus);
        if let Some(closed) = closed {
            closed.restore_focus(window, cx);
        }
        if opened {
            window.focus(&focus_target, cx);
        }
        window.refresh();
    }

    /// 全屏点击拦截与锚定弹层。
    pub fn overlay(
        &self,
        window: &Window,
        cx: &App,
        content: impl IntoElement,
    ) -> impl IntoElement {
        self.overlay_inner(window, cx, content, true, None)
    }

    /// 菜单弹层使用外部点击事件关闭，保留顶栏相邻菜单的鼠标命中。
    pub fn menu_overlay(
        &self,
        window: &Window,
        cx: &App,
        content: impl IntoElement,
        on_outside: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        self.overlay_inner(window, cx, content, false, Some(Box::new(on_outside)))
    }

    fn overlay_inner(
        &self,
        window: &Window,
        cx: &App,
        content: impl IntoElement,
        intercept_background: bool,
        on_outside: Option<OutsideClickHandler>,
    ) -> impl IntoElement {
        let id = self.id;
        let layer = self.layer.clone();
        let win_size = window.bounds().size;
        let mut panel = div()
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
            });
        if let Some(on_outside) = on_outside {
            panel = panel.on_mouse_down_out(on_outside);
        }

        div()
            .when(intercept_background, |overlay| {
                overlay.child(
                    deferred(
                        div()
                            .absolute()
                            .top(Pixels::ZERO)
                            .left(Pixels::ZERO)
                            .w(win_size.width)
                            .h(win_size.height)
                            .occlude()
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                if let Some(active) = layer.dismiss(id) {
                                    active.restore_focus(window, cx);
                                }
                                window.refresh();
                            }),
                    )
                    .with_priority(0),
                )
            })
            .child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopLeft)
                        .position(point(Pixels::ZERO, Pixels::ZERO))
                        .position_mode(gpui::AnchoredPositionMode::Local)
                        .snap_to_window_with_margin(scale::to_pixels(scale::S6, window))
                        .child(
                            panel.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .max_h(
                                        win_size.height - scale::to_pixels(scale::S6, window) * 2.0,
                                    )
                                    .bg(color::current(cx).elevated_surface_background)
                                    .border_l_3()
                                    .border_color(color::current(cx).border_focused)
                                    .border_1()
                                    .border_color(color::current(cx).border)
                                    .rounded_lg()
                                    .overflow_hidden()
                                    .child(content),
                            ),
                        ),
                )
                .with_priority(1),
            )
    }
}
