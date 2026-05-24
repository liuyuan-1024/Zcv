use std::rc::Rc;

use gpui::{Div, div, prelude::*};

use crate::shell::editor::{EditorKind, TextEditorSlot};
use crate::shell::shared::theme::{color, typography};

use crate::shell::features::project_picker::{ProjectPickerMode, ProjectPickerState};

pub(super) fn render(state: &ProjectPickerState, slot: &Rc<TextEditorSlot>) -> Div {
    input_box(state, slot)
}

fn input_box(state: &ProjectPickerState, slot: &Rc<TextEditorSlot>) -> Div {
    let placeholder = match state.mode {
        ProjectPickerMode::Browse => "搜索项目...",
        ProjectPickerMode::CloneGit => "输入 Git 仓库地址...",
    };

    let mut box_ = div()
        .relative()
        .w_full()
        .h(typography::editor_line())
        .flex()
        .items_center()
        .text_size(typography::editor())
        .line_height(typography::editor_line())
        .text_color(color::gray::g95())
        .overflow_hidden();

    if state.query.text.is_empty() {
        box_ = box_.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .text_color(color::gray::g60())
                .child(placeholder),
        );
    }

    box_ = box_.child(slot.embed(EditorKind::SingleLine));

    box_
}
