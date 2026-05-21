//! 主编辑器网格：多行正文、光标显示、键盘与输入宿主注册。
//!
//! 数据仍来自 workspace buffer + active view 的快照；本模块只拥有编辑器表面
//! 的交互和绘制。文件生命周期、标签、dirty 等仍留给 workspace / workbench。

use gpui::{Div, FocusHandle, MouseButton, canvas, div, prelude::*};

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
    let lines = visual_lines(&state.text, state.cursor_byte);
    div()
        .flex_1()
        .overflow_hidden()
        .rounded(theme::radius::r4())
        .bg(color::gray::g00())
        .p(space::s12())
        .font_family(".ZedMono")
        .line_height(typography::editor_line())
        .text_size(typography::editor())
        // editor 渲染区每帧重新注册一次系统输入法接收端：bounds 给候选窗定位用。
        // handle_input 必须在 paint 阶段调用，所以放进 canvas 的第二个回调而非 prepaint。
        .child(
            canvas(
                |bounds, _, _| bounds,
                move |_, bounds, window, cx| input_handler_hook(bounds, window, cx),
            )
            .size_full()
            .absolute(),
        )
        .children(lines.into_iter().enumerate().map(|(index, line)| {
            div()
                .flex()
                .flex_row()
                .gap(space::s12())
                .child(
                    // 行号为次级信息，与标题栏状态同用 g60。
                    div()
                        .w(space::s24())
                        .text_color(color::gray::g60())
                        .child((index + 1).to_string()),
                )
                .child(
                    // 代码正文与 top bar 文本同基线 g75。
                    div()
                        .flex_1()
                        .whitespace_nowrap()
                        .text_color(color::gray::g75())
                        .child(line),
                )
        }))
}

fn visual_lines(text: &str, cursor_byte: usize) -> Vec<String> {
    if text.is_empty() {
        return vec!["|".to_string()];
    }

    let cursor_byte = cursor_byte.min(text.len());
    let mut lines = Vec::new();
    let mut line_start = 0;

    for (line_index, raw_line) in text.split('\n').enumerate() {
        let line_end = line_start + raw_line.len();
        let mut line = raw_line.to_string();
        if cursor_byte >= line_start && cursor_byte <= line_end {
            let column = cursor_byte - line_start;
            line.insert(column, '|');
        } else if line_index == 0 && cursor_byte == 0 {
            line.insert(0, '|');
        }
        lines.push(line);
        line_start = line_end + 1;
    }

    lines
}
