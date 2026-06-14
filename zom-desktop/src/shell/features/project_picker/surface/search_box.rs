use std::rc::Rc;

use gpui::{Div, div, prelude::*};

use crate::editor::TextEditorSlot;
use crate::theme::{color, space, typography};

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
        .border_color(color::current().gray.s05)
        .text_color(color::current().gray.s09)
        .child(
            div()
                .flex_1()
                .relative()
                .flex()
                .items_center()
                .overflow_hidden()
                .child(editor(
                    slot,
                    state
                        .query
                        .lines
                        .first()
                        .map(|line| line.text.is_empty())
                        .unwrap_or(true),
                    placeholder,
                )),
        )
}

fn editor(slot: &Rc<TextEditorSlot>, show_placeholder: bool, placeholder: &'static str) -> Div {
    let mut editor = div()
        .relative()
        .h(typography::ui_line())
        .flex_1()
        .overflow_hidden()
        .text_color(color::current().gray.s09);

    if show_placeholder {
        editor = editor.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .text_color(color::current().gray.s08)
                .child(placeholder),
        );
    }

    editor.child(slot.embed())
}
