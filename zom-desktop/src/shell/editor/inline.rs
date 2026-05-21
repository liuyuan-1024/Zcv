//! 内联编辑器渲染与输入宿主注册。

use gpui::{Div, canvas, div, prelude::*, px};

use crate::shell::InputHandlerHook;
use crate::shell::shared::theme::{color, typography};

use super::core::EditorSnapshot;

/// 渲染单行内联编辑器：文本 + 光标 + 系统输入法接收端。
/// 业务图标、缩进、边框由消费方决定。
pub(crate) fn render_inline(
    snapshot: &EditorSnapshot,
    input_handler_hook: &InputHandlerHook,
) -> Div {
    let cursor_byte = snapshot.cursor_byte.min(snapshot.text.len());
    let before = snapshot.text.get(..cursor_byte).unwrap_or("");
    let after = snapshot
        .text
        .get(cursor_byte..)
        .unwrap_or(snapshot.text.as_str());
    div()
        .flex_1()
        .flex()
        .flex_row()
        .items_center()
        .relative()
        .overflow_hidden()
        .child(
            canvas(|bounds, _, _| bounds, {
                let input_handler_hook = input_handler_hook.clone();
                move |_, bounds, window, cx| input_handler_hook(bounds, window, cx)
            })
            .size_full()
            .absolute(),
        )
        .child(div().child(before.to_string()))
        .child(caret())
        .child(div().child(after.to_string()))
}

fn caret() -> Div {
    div()
        .flex_shrink_0()
        .w(px(1.0))
        .h(typography::ui_line())
        .bg(color::focus::border())
}
