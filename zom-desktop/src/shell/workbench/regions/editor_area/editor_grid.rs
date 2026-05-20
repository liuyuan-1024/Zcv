//! EditorGrid —— 主编辑区 L4 Region（布局模型 4.6 / 手册 19 / 20.9）。
//!
//! 不走 Panel 模型。第一版先渲染单 view，让键盘输入通过 command
//! 管线进入 engine；split tree 与 tab group 后续按 19 章展开。

use gpui::{Div, FocusHandle, MouseButton, canvas, div, prelude::*};

use crate::shell::shared::theme::{self, color, space, typography};
use crate::shell::workbench::state::EditorState;
use crate::shell::{InputHandlerHook, KeyRequest, normalized_chord};

pub(crate) fn render(
    state: &EditorState,
    key_request: KeyRequest,
    input_handler_hook: InputHandlerHook,
    editor_focus: FocusHandle,
) -> Div {
    let focus_for_click = editor_focus.clone();
    let key_handler = key_request;

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
        .child(editor_surface(state, input_handler_hook))
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
