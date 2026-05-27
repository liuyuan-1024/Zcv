use std::rc::Rc;

use gpui::{Div, div, prelude::*};

use crate::shell::editor::TextEditorSlot;
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
        .flex()
        .items_center()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::gray::s09())
        .overflow_hidden();

    if state.query.text().is_empty() {
        box_ = box_.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .text_color(color::gray::s08())
                .child(placeholder),
        );
    }

    box_ = box_.child(slot.embed());

    box_
}
