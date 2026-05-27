use gpui::{Div, div, prelude::*};
use zom_command::commands::project_picker as project_picker_commands;

use crate::shell::shared::theme::{color, space, typography};

use super::{ProjectPickerActions, ProjectPickerMode, command_shortcut, command_title};

pub(super) fn render(mode: ProjectPickerMode, actions: &ProjectPickerActions) -> Div {
    div()
        .flex()
        .flex_col()
        .p(space::s6())
        .child(source_action_row(
            command_title(actions, project_picker_commands::OPEN_LOCAL_PROJECT),
            command_shortcut(actions, project_picker_commands::OPEN_LOCAL_PROJECT),
        ))
        .child(source_action_row(
            command_title(actions, project_picker_commands::START_GIT_CLONE),
            git_clone_hint(mode, actions),
        ))
}

fn git_clone_hint(mode: ProjectPickerMode, actions: &ProjectPickerActions) -> String {
    if mode == ProjectPickerMode::CloneGit {
        command_shortcut(actions, project_picker_commands::ACTIVATE)
    } else {
        command_shortcut(actions, project_picker_commands::START_GIT_CLONE)
    }
}

fn source_action_row(label: String, hint: String) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .text_size(typography::ui())
        .text_color(color::gray::s09())
        .child(div().truncate().child(label))
        .child(
            div()
                .flex_shrink_0()
                .text_color(color::gray::s08())
                .child(hint),
        )
}
