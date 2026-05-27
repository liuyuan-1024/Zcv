//! 主编辑区面板 —— 围绕可嵌入编辑器元素的工作台外壳。
//!
//! 外壳（键盘焦点宿主、背景 / 圆角 / 内边距、无文件时的空态）属于工作台
//! 编辑区，不属于编辑器本身：焦点与按键路由随交互面（`KeySurface::Editor`）
//! 而定，编辑器只是被嵌进来的那个子元素。

use std::rc::Rc;

use gpui::{Div, FocusHandle, MouseButton, div, prelude::*};

use crate::shell::editor::TextEditorSlot;
use crate::shell::shared::theme::{self, color, space, typography};
use crate::shell::workbench::state::EditorState;
use crate::shell::{KeyRequest, normalized_chord};

/// 渲染主编辑区面板：焦点宿主 + 编辑器（或无文件空态）。
pub(super) fn render(
    state: &EditorState,
    key_request: KeyRequest,
    editor_slot: Rc<TextEditorSlot>,
    editor_focus: FocusHandle,
) -> Div {
    let focus_for_click = editor_focus.clone();

    // 无活动文件时给一句提示，而不是渲染一个空编辑器 —— 与文件树未打开
    // 项目时的占位口径一致。焦点宿主（track_focus + on_key_down）两态都挂。
    let body = if state.tabs.is_empty() {
        empty_message("尚未打开文件")
    } else {
        editor_surface(&editor_slot)
    };

    div()
        .flex_1()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(color::gray::s02())
        .text_color(color::gray::s09())
        .track_focus(&editor_focus)
        .tab_index(0)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&focus_for_click);
            cx.stop_propagation();
        })
        .on_key_down(move |event, window, cx| {
            // 只在 keymap 命中 / 等待 leader 续击时拦截事件；NoMatch 必须放行，
            // 否则 macOS 把 propagate=false 当作"已处理"，NSTextInputClient
            // 永远拿不到输入，系统输入法直接哑掉。
            if key_request(normalized_chord(&event.keystroke), window, cx) {
                cx.stop_propagation();
            }
        })
        .child(body)
}

/// 无活动文件时的占位提示，居中铺满编辑区。
fn empty_message(hint: &'static str) -> Div {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_size(typography::ui())
        .text_color(color::gray::s09())
        .child(hint)
}

/// 编辑器嵌入面：设定代码正文的文本样式，子元素是主编辑区嵌入 slot。
fn editor_surface(slot: &Rc<TextEditorSlot>) -> Div {
    // 文本样式（mono 字体 / 字号 / 行高 / 正文色）在此设定，由 slot 渲染的
    // 编辑器元素继承。行号色、光标色是编辑器自持的视觉角色，不在此设。
    div()
        .flex_1()
        .overflow_hidden()
        .rounded(theme::radius::r4())
        .bg(color::gray::s01())
        .p(space::s12())
        .font(typography::editor_font())
        .line_height(typography::editor_line())
        .text_size(typography::editor())
        .text_color(color::gray::s09())
        .child(slot.embed())
}
