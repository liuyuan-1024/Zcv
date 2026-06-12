//! 删除确认模态弹窗。
//!
//! 居中浮层 + 半透明遮罩。键盘的「确认 / 取消」由文件树在 `PendingDelete`
//! 键模式下处理（Enter / Esc），本弹窗只负责视觉与鼠标按钮。

use gpui::{AnyElement, MouseButton, Rgba, black, deferred, div, prelude::*, px};
use zom_workspace::EntryKind;

use super::{ConfirmDeleteHandlers, PendingDelete};
use crate::host_intent::CommandRequest;
use crate::theme::{color, radius, space, typography};

/// 渲染居中删除确认弹窗。`deferred` + 高优先级让它压在所有面板与锚定
/// surface（priority 30）之上。
pub(super) fn render(pending: &PendingDelete, handlers: &ConfirmDeleteHandlers) -> AnyElement {
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
            .child(dialog(pending, handlers)),
    )
    .priority(50)
    .into_any_element()
}

fn dialog(pending: &PendingDelete, handlers: &ConfirmDeleteHandlers) -> impl IntoElement {
    // 三种文案：
    // - 单删文件：明示文件名
    // - 单删目录：明示并提示"及其全部内容"
    // - 多删：以首项命名 + "等 N 项"；若集合中含目录，强调"内容也会一并移走"
    let message = if pending.count == 1 {
        match pending.first_kind {
            EntryKind::File => {
                format!("确认把文件「{}」移到系统回收站？", pending.first_name)
            }
            EntryKind::Directory => format!(
                "确认把目录「{}」及其全部内容移到系统回收站？",
                pending.first_name
            ),
        }
    } else if pending.has_directory {
        format!(
            "确认把「{}」等 {} 项（含目录）及其全部内容移到系统回收站？",
            pending.first_name, pending.count
        )
    } else {
        format!(
            "确认把「{}」等 {} 项移到系统回收站？",
            pending.first_name, pending.count
        )
    };
    div()
        .w(px(360.0))
        .flex()
        .flex_col()
        .gap(space::s6())
        .p(space::s6())
        .rounded(radius::r4())
        .border_1()
        .border_color(color::current().gray.s05)
        .bg(color::current().gray.s03)
        // 吞掉对话框内的按下，避免冒泡到遮罩误触发取消。
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .text_size(typography::ui())
                .text_color(color::current().gray.s09)
                .child("移到回收站"),
        )
        .child(
            div()
                .text_size(typography::ui())
                .text_color(color::current().gray.s09)
                .child(message),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(space::s6())
                .child(button(
                    "confirm-delete.cancel",
                    "取消",
                    color::current().gray.s09,
                    handlers.cancel.clone(),
                ))
                .child(button(
                    "confirm-delete.confirm",
                    "删除",
                    color::current().red.s07,
                    handlers.confirm.clone(),
                )),
        )
}

fn button(
    id: &'static str,
    label: &'static str,
    text_color: Rgba,
    action: CommandRequest,
) -> impl IntoElement {
    div()
        .id(id)
        .p(space::s6())
        .rounded(radius::r4())
        .bg(color::current().gray.s04)
        .text_size(typography::ui())
        .text_color(text_color)
        .cursor_pointer()
        .on_click(move |_, window, cx| action(window, cx))
        .child(label)
}
