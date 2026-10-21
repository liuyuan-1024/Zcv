//! PickerHost —— 「按钮 + 浮层」外壳：打开状态、dismiss、焦点与浮层渲染。
//!
//! ProjectPicker / BranchPicker 各自持有数据源与按钮内容，共用的开关交互、焦点管理、全屏拦截与浮层样式链收敛到这里。
//! 打开状态与互斥由同一顶栏持有的 `PickerLayer` 管理，常驻输入入口也复用它的焦点会话。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::{
    Anchor, App, Entity, FocusHandle, MouseButton, Pixels, WeakFocusHandle, Window, anchored,
    deferred, div, point, prelude::*,
};
use zcv_theme::{color, scale};

use crate::picker_view::OnDismiss;
use crate::{Picker, PickerDelegate};

/// 浮层身份：同身份重复打开 = 关闭（toggle 语义）。
#[derive(Clone, Copy, PartialEq, Eq)]
struct OverlayId(usize);

impl OverlayId {
    fn next() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// 同一顶栏的入口与选择器共享一个活动浮层；其他窗口拥有独立实例。
#[derive(Clone, Default)]
pub struct PickerLayer {
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

impl PickerLayer {
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

/// 顶栏浮层与焦点会话外壳；选择器另外使用按钮与浮层渲染能力。
pub struct PickerHost {
    id: OverlayId,
    focus: FocusHandle,
    layer: PickerLayer,
}

impl PickerHost {
    pub fn new(focus: FocusHandle, layer: PickerLayer) -> Self {
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

    /// 供 Picker 的 on_dismiss 使用的处理器。
    pub fn on_dismiss_handler(&self) -> OnDismiss {
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

    /// 切换开合：打开时聚焦 Picker 搜索框，关闭时把焦点还给发起位置。
    /// 调用方在打开分支先刷新数据源，再调用本方法。
    pub fn toggle<D: PickerDelegate>(
        &mut self,
        picker: &Entity<Picker<D>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_scope = picker.read(cx).focus_handle();
        let previous_focus = window.focused(cx).map(|focus| focus.downgrade());
        let (opened, closed) = self.layer.toggle(self.id, focus_scope, previous_focus);
        if let Some(closed) = closed {
            closed.restore_focus(window, cx);
        }
        if opened {
            let input = picker.read(cx).search_input().clone();
            window.focus(&input.focus_handle(cx), cx);
        }
        window.refresh();
    }

    /// 浮层：全屏点击拦截（垫底）+ anchored 弹层（包裹 Picker）。
    pub fn overlay<D: PickerDelegate>(
        &self,
        window: &Window,
        cx: &App,
        picker: &Entity<Picker<D>>,
    ) -> impl IntoElement {
        let id = self.id;
        let layer = self.layer.clone();
        let win_size = window.bounds().size;

        div()
            .child(
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
            .child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopLeft)
                        .position(point(Pixels::ZERO, Pixels::ZERO))
                        .position_mode(gpui::AnchoredPositionMode::Local)
                        .snap_to_window_with_margin(scale::to_pixels(scale::S6, window))
                        .child(
                            div()
                                .occlude()
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .max_h(
                                            win_size.height
                                                - scale::to_pixels(scale::S6, window) * 2.0,
                                        )
                                        .bg(color::current(cx).elevated_surface_background)
                                        .border_l_3()
                                        .border_color(color::current(cx).border_focused)
                                        .border_1()
                                        .border_color(color::current(cx).border)
                                        .rounded_lg()
                                        .overflow_hidden()
                                        .child(picker.clone()),
                                ),
                        ),
                )
                .with_priority(1),
            )
    }
}
