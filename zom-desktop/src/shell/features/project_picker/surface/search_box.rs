use std::rc::Rc;

use gpui::{Div, div, prelude::*};

use crate::shell::editor::TextEditorSlot;
use crate::shell::shared::theme::{color, space, typography};

use crate::shell::features::project_picker::{ProjectPickerMode, ProjectPickerState};

pub(super) fn render(state: &ProjectPickerState, slot: &Rc<TextEditorSlot>) -> Div {
    input_box(state, slot)
}

fn input_box(state: &ProjectPickerState, slot: &Rc<TextEditorSlot>) -> Div {
    let placeholder = match state.mode {
        ProjectPickerMode::Browse => "搜索项目...",
        ProjectPickerMode::CloneGit => "输入 Git 仓库地址...",
    };

    div()
        .w_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .p(space::s6())
        .border_b_1()
        .border_color(color::gray::s05())
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::gray::s09())
        .child(
            div()
                .flex_1()
                .relative()
                .flex()
                .items_center()
                .overflow_hidden()
                .child(editor(slot, state.query.text().is_empty(), placeholder)),
        )
}

fn editor(slot: &Rc<TextEditorSlot>, show_placeholder: bool, placeholder: &'static str) -> Div {
    let mut editor = div()
        .relative()
        .h(typography::ui_line())
        .flex_1()
        .overflow_hidden()
        .text_color(color::gray::s09());

    if show_placeholder {
        editor = editor.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .text_color(color::gray::s08())
                .child(placeholder),
        );
    }

    editor.child(slot.embed())
}
