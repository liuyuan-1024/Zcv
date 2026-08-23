//! PickerHost —— 「按钮 + 浮层」外壳：打开状态、dismiss、焦点与浮层渲染。
//!
//! ProjectPicker / BranchPicker 各自持有数据源与 glyph 内容，共用的开关交互、焦点管理、全屏拦截与浮层样式链收敛到这里。

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Corner, Entity, FocusHandle, MouseButton, Pixels, Window, anchored, deferred, div, point,
    prelude::*, px,
};
use zcv_theme::{color, space};

use crate::picker_view::OnDismiss;
use crate::{Picker, PickerDelegate};

/// 「按钮 + 浮层」外壳。
pub struct PickerHost {
    is_open: bool,
    dismiss_flag: Rc<Cell<bool>>,
    focus: FocusHandle,
}

impl PickerHost {
    pub fn new(focus: FocusHandle) -> Self {
        Self {
            is_open: false,
            dismiss_flag: Rc::new(Cell::new(false)),
            focus,
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 供 Picker 的 on_dismiss 使用的处理器：置位 dismiss 标志并请求重绘。
    pub fn on_dismiss_handler(&self) -> OnDismiss {
        let dismiss = self.dismiss_flag.clone();
        Box::new(move |window, _app| {
            dismiss.set(true);
            window.refresh();
        })
    }

    /// 关闭浮层并把焦点还给按钮（dismiss 或外部关闭时调用）。
    pub fn close_and_refocus(&mut self, window: &mut Window) {
        self.is_open = false;
        window.focus(&self.focus);
    }

    /// 切换开合：打开时聚焦 Picker 搜索框，关闭时焦点还给按钮。
    /// 调用方在打开分支先刷新数据源，再调用本方法。
    pub fn toggle<D: PickerDelegate>(
        &mut self,
        picker: &Entity<Picker<D>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.dismiss_flag.set(false);
        self.is_open = !self.is_open;
        if self.is_open {
            if let Some(input) = picker.read(cx).search_input().cloned() {
                let focus = input.focus_handle(cx);
                window.focus(&focus);
            }
        } else {
            window.focus(&self.focus);
        }
        window.refresh();
    }

    /// render 时消费 dismiss 标志；返回 true 表示本次应关闭浮层。
    pub fn consume_dismiss(&mut self) -> bool {
        self.dismiss_flag.replace(false)
    }

    /// 浮层：全屏点击拦截（垫底）+ anchored 弹层（包裹 Picker）。
    pub fn overlay<D: PickerDelegate>(
        &self,
        window: &Window,
        cx: &App,
        picker: &Entity<Picker<D>>,
    ) -> impl IntoElement {
        let dismiss = self.dismiss_flag.clone();
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
                        .on_mouse_down(MouseButton::Left, move |_, window, _cx| {
                            dismiss.set(true);
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
