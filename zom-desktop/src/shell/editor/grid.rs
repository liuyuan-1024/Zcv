//! 主编辑器网格：多行正文、光标显示、键盘与输入宿主注册。
//!
//! 数据仍来自 workspace buffer + active view 的快照；本模块只拥有编辑器表面
//! 的交互和绘制。文件生命周期、标签、dirty 等仍留给 workspace / workbench。

use gpui::{Div, FocusHandle, MouseButton, div, prelude::*};

use super::element::EditorElement;
use crate::shell::shared::theme::{self, color, space, typography};
use crate::shell::workbench::state::EditorState;
use crate::shell::{InputHandlerHook, KeyRequest, normalized_chord};

pub(crate) fn render_grid(
    state: &EditorState,
    key_request: KeyRequest,
    input_handler_hook: InputHandlerHook,
    editor_focus: FocusHandle,
) -> Div {
    let focus_for_click = editor_focus.clone();
    let key_handler = key_request;

    // 无活动文件时给一句提示，而不是渲染一个空编辑区 —— 与文件树未打开
    // 项目时的占位口径一致。焦点宿主（track_focus + on_key_down）两态都挂。
    let body = if state.tabs.is_empty() {
        empty_message("尚未打开文件")
    } else {
        editor_surface(state, input_handler_hook)
    };

    div()
        .flex_1()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(color::gray::g05())
        .text_color(color::gray::g90())
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
            if key_handler(normalized_chord(&event.keystroke), window, cx) {
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
        .text_color(color::gray::g75())
        .child(hint)
}

fn editor_surface(state: &EditorState, input_handler_hook: InputHandlerHook) -> Div {
    // 文本样式（mono 字体 / 字号 / 行高 / 正文色）在此设定，由 EditorElement
    // 继承；行号 g60、正文 g75 与旧布局一致。文本与光标分层绘制见 EditorElement。
    div()
        .flex_1()
        .overflow_hidden()
        .rounded(theme::radius::r4())
        .bg(color::gray::g00())
        .p(space::s12())
        .font_family(".ZedMono")
        .line_height(typography::editor_line())
        .text_size(typography::editor())
        .text_color(color::gray::g75())
        .child(
            EditorElement::new(state.text.clone(), state.cursor_byte, input_handler_hook)
                .with_gutter(color::gray::g60())
                .caret_color(color::focus::border()),
        )
}
