//! PickerHost —— 「按钮 + 浮层」外壳：打开状态、dismiss、焦点与浮层渲染。
//!
//! ProjectPicker / BranchPicker 各自持有数据源与 glyph 内容，共用的开关交互、焦点管理、全屏拦截与浮层样式链收敛到这里。
//! 打开状态与互斥统一由全局 `ModalLayer` 管理（同一时刻最多一个浮层）。

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::{
    App, Corner, Entity, FocusHandle, Global, MouseButton, Pixels, Window, anchored, deferred, div,
    point, prelude::*, px,
};
use zcv_theme::{color, space};

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

/// 全局浮层互斥层：同一时刻最多一个浮层（对齐 Zed 的 ModalLayer 单例语义）。
#[derive(Default)]
struct ModalLayer {
    active: Option<ActiveOverlay>,
}

struct ActiveOverlay {
    id: OverlayId,
    /// 垫层点击或互斥关闭时置位；浮层渲染时消费并关闭自己。
    dismiss: Rc<Cell<bool>>,
    /// 同浮层重复打开（toggle 关闭）时的副作用：焦点归还按钮。
    on_close: Rc<dyn Fn(&mut Window)>,
}

impl Global for ModalLayer {}

impl ModalLayer {
    /// 切换开合：同身份重复打开 = 关闭（焦点归还按钮）；
    /// 其他浮层打开时先置其关闭标志（互斥，焦点由新浮层接管）。
    /// 返回是否处于打开状态。
    fn toggle(
        &mut self,
        id: OverlayId,
        on_close: Rc<dyn Fn(&mut Window)>,
        window: &mut Window,
    ) -> bool {
        match self.active.take() {
            Some(active) if active.id == id => {
                (active.on_close)(window);
                false
            }
            Some(active) => {
                active.dismiss.set(true);
                self.active = Some(ActiveOverlay {
                    id,
                    dismiss: Rc::new(Cell::new(false)),
                    on_close,
                });
                true
            }
            None => {
                self.active = Some(ActiveOverlay {
                    id,
                    dismiss: Rc::new(Cell::new(false)),
                    on_close,
                });
                true
            }
        }
    }

    /// 垫层点击 / Esc 关闭：置位关闭标志，由浮层渲染时消费。
    fn dismiss(&mut self, id: OverlayId) {
        if let Some(active) = &self.active
            && active.id == id
        {
            active.dismiss.set(true);
        }
    }

    /// 渲染时消费关闭标志：置位则清出活动层并返回 true（本次应关闭浮层）。
    fn consume_dismiss(&mut self, id: OverlayId) -> bool {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.id == id && active.dismiss.replace(false))
        {
            self.active = None;
            true
        } else {
            false
        }
    }

    /// 当前活动浮层身份（互斥关闭时供被顶掉的浮层判断是否让出焦点）。
    fn active_id(&self) -> Option<OverlayId> {
        self.active.as_ref().map(|active| active.id)
    }
}

/// 写访问 ModalLayer：首次使用前完成初始化。
fn with_modal_layer<T>(cx: &mut App, f: impl FnOnce(&mut ModalLayer) -> T) -> T {
    if cx.try_global::<ModalLayer>().is_none() {
        cx.set_global(ModalLayer::default());
    }
    cx.update_global::<ModalLayer, _>(|layer, _| f(layer))
}

/// 「按钮 + 浮层」外壳。
pub struct PickerHost {
    id: OverlayId,
    focus: FocusHandle,
}

impl PickerHost {
    pub fn new(focus: FocusHandle) -> Self {
        Self {
            id: OverlayId::next(),
            focus,
        }
    }

    pub fn is_open(&self, cx: &App) -> bool {
        cx.try_global::<ModalLayer>()
            .is_some_and(|layer| layer.active_id() == Some(self.id))
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 供 Picker 的 on_dismiss 使用的处理器：置位关闭标志并请求重绘。
    pub fn on_dismiss_handler(&self) -> OnDismiss {
        let id = self.id;
        Box::new(move |window, app| {
            with_modal_layer(app, |layer| layer.dismiss(id));
            window.refresh();
        })
    }

    /// 关闭浮层并把焦点还给按钮。
    ///
    /// 被其他浮层顶掉（互斥关闭）时不让出焦点——焦点已由新浮层接管。
    pub fn close_and_refocus(&mut self, window: &mut Window, cx: &App) {
        let superseded = cx
            .try_global::<ModalLayer>()
            .is_some_and(|layer| layer.active_id().is_some_and(|active| active != self.id));
        if !superseded {
            window.focus(&self.focus);
        }
    }

    /// 切换开合：打开时聚焦 Picker 搜索框，关闭时焦点还给按钮。
    /// 调用方在打开分支先刷新数据源，再调用本方法。
    pub fn toggle<D: PickerDelegate>(
        &mut self,
        picker: &Entity<Picker<D>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.focus.clone();
        let opened = with_modal_layer(cx, |layer| {
            layer.toggle(
                self.id,
                Rc::new(move |window| {
                    window.focus(&focus);
                }),
                window,
            )
        });
        if opened && let Some(input) = picker.read(cx).search_input().cloned() {
            window.focus(&input.focus_handle(cx));
        }
        window.refresh();
    }

    /// render 时消费关闭标志；返回 true 表示本次应关闭浮层。
    pub fn consume_dismiss(&self, cx: &mut App) -> bool {
        with_modal_layer(cx, |layer| layer.consume_dismiss(self.id))
    }

    /// 浮层：全屏点击拦截（垫底）+ anchored 弹层（包裹 Picker）。
    pub fn overlay<D: PickerDelegate>(
        &self,
        window: &Window,
        cx: &App,
        picker: &Entity<Picker<D>>,
    ) -> impl IntoElement {
        let id = self.id;
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
                            with_modal_layer(cx, |layer| layer.dismiss(id));
                            window.refresh();
                        }),
                )
                .with_priority(0),
            )
            .child(
                deferred(
                    anchored()
                        .anchor(Corner::TopLeft)
                        .position(point(Pixels::ZERO, Pixels::ZERO))
                        .position_mode(gpui::AnchoredPositionMode::Local)
                        .snap_to_window_with_margin(space::S6)
                        .child(
                            div()
                                .occlude()
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .child(
                                    div()
                                        .bg(color::current(cx).elevated_surface_background)
                                        .border_l_3()
                                        .border_color(color::current(cx).border_focused)
                                        .border_1()
                                        .border_color(color::current(cx).border_variant)
                                        .rounded(px(8.0))
                                        .overflow_hidden()
                                        .child(picker.clone()),
                                ),
                        ),
                )
                .with_priority(1),
            )
    }
}
