use gpui::{Div, div, prelude::*};

use crate::shell::shared::theme::{color, typography};

use super::{PickerMode, ProjectPickerState};

pub(super) fn render(state: &ProjectPickerState) -> Div {
    input_box(state)
}

fn input_box(state: &ProjectPickerState) -> Div {
    let placeholder = match state.mode {
        PickerMode::Browse => "搜索项目...",
        PickerMode::CloneGit => "输入 Git 仓库地址...",
    };
    let text = if state.query.is_empty() {
        placeholder.to_string()
    } else {
        state.query.clone()
    };
    let text_color = if state.query.is_empty() {
        color::gray::g60()
    } else {
        color::gray::g95()
    };

    div()
        .w_full()
        .flex()
        .items_center()
        .text_size(typography::editor())
        .text_color(text_color)
        .overflow_hidden()
        .child(div().truncate().child(text))
}
