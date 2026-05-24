//! 删除确认模态弹窗。
//!
//! 居中浮层 + 半透明遮罩。键盘的「确认 / 取消」由文件树在 `PendingDelete`
//! 键模式下处理（Enter / Esc），本弹窗只负责视觉与鼠标按钮。

use gpui::{AnyElement, MouseButton, Rgba, black, deferred, div, prelude::*, px};
use zom_workspace::EntryKind;

use super::ConfirmDeleteHandlers;
use crate::shell::ActionRequest;
use crate::shell::shared::theme::{color, radius, space, typography};

/// 渲染居中删除确认弹窗。`deferred` + 高优先级让它压在所有面板与锚定
/// surface（priority 30）之上。
pub(super) fn render(name: &str, kind: EntryKind, handlers: &ConfirmDeleteHandlers) -> AnyElement {
    let cancel_on_scrim = handlers.cancel.clone();
    deferred(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(black().opacity(0.45))
            // 点击遮罩等于取消；并拦下事件，避免穿透到下层面板。
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                cancel_on_scrim(window, cx);
            })
            .child(dialog(name, kind, handlers)),
    )
    .priority(50)
    .into_any_element()
}

fn dialog(name: &str, kind: EntryKind, handlers: &ConfirmDeleteHandlers) -> impl IntoElement {
    // 目录会连同其全部内容一并删除，措辞上明确提示。
    let message = match kind {
        EntryKind::File => format!("确认把文件「{name}」移到系统回收站？"),
        EntryKind::Directory => {
            format!("确认把目录「{name}」及其全部内容移到系统回收站？")
        }
    };
    div()
        .w(px(360.0))
        .flex()
        .flex_col()
        .gap(space::s12())
        .p(space::s16())
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::g40())
        .bg(color::gray::g10())
        // 吞掉对话框内的按下，避免冒泡到遮罩误触发取消。
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .text_size(typography::ui())
                .text_color(color::gray::g95())
                .child("移到回收站"),
        )
        .child(
            div()
                .text_size(typography::ui())
                .text_color(color::gray::g75())
                .child(message),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(space::s8())
                .child(button(
                    "confirm-delete.cancel",
                    "取消",
                    color::gray::g90(),
                    handlers.cancel.clone(),
                ))
                .child(button(
                    "confirm-delete.confirm",
                    "删除",
                    color::accent::danger(),
                    handlers.confirm.clone(),
                )),
        )
}

fn button(
    id: &'static str,
    label: &'static str,
    text_color: Rgba,
    action: ActionRequest,
) -> impl IntoElement {
    div()
        .id(id)
        .px(space::s12())
        .py(space::s4())
        .rounded(radius::r4())
        .bg(color::gray::g20())
        .text_size(typography::ui())
        .text_color(text_color)
        .cursor_pointer()
        .on_click(move |_, window, cx| action(window, cx))
        .child(label)
}
