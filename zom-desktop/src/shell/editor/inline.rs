//! 内联编辑器渲染：单行文本 + 光标 + 系统输入法接收端。
//!
//! 与主编辑网格共用 [`EditorElement`]：文本与光标分层绘制，移动光标不重排
//! 文字。业务图标、缩进、边框由消费方决定。

use gpui::{Div, div, prelude::*};

use super::core::EditorSnapshot;
use super::element::EditorElement;
use crate::shell::InputHandlerHook;
use crate::shell::shared::theme::{color, typography};

/// 渲染单行内联编辑器。文本字体 / 字号 / 颜色从父级继承，行高对齐 UI 行尺寸。
pub(crate) fn render_inline(
    snapshot: &EditorSnapshot,
    input_handler_hook: &InputHandlerHook,
) -> Div {
    div()
        .flex_1()
        .flex()
        .items_center()
        .overflow_hidden()
        .line_height(typography::ui_line())
        .child(
            EditorElement::new(
                snapshot.text.clone(),
                snapshot.cursor_byte,
                input_handler_hook.clone(),
            )
            .caret_color(color::focus::border()),
        )
}
