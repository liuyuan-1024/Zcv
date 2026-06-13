use std::rc::Rc;

use gpui::{Div, MouseButton, div, prelude::*};
use zom_command::commands::project_picker as project_picker_commands;

use crate::shell::features::project_picker::ProjectPickerIntent;
use crate::theme::{color, space, typography};

use super::{ProjectPickerActions, ProjectPickerMode, command_shortcut, command_title};

pub(super) fn render(mode: ProjectPickerMode, actions: &ProjectPickerActions) -> Div {
    let intent_request = Rc::clone(&actions.intent_request);
    div()
        .flex()
        .flex_col()
        .p(space::s6())
        .child(source_action_row(
            command_title(actions, project_picker_commands::OPEN_LOCAL_PROJECT),
            command_shortcut(actions, project_picker_commands::OPEN_LOCAL_PROJECT),
            ProjectPickerIntent::OpenLocalProject,
            Rc::clone(&intent_request),
        ))
        .child(source_action_row(
            command_title(actions, project_picker_commands::START_GIT_CLONE),
            git_clone_hint(mode, actions),
            ProjectPickerIntent::StartGitClone,
            intent_request,
        ))
}

fn git_clone_hint(mode: ProjectPickerMode, actions: &ProjectPickerActions) -> String {
    if mode == ProjectPickerMode::CloneGit {
        command_shortcut(actions, project_picker_commands::ACTIVATE)
    } else {
        command_shortcut(actions, project_picker_commands::START_GIT_CLONE)
    }
}

fn source_action_row(
    label: String,
    hint: String,
    intent: ProjectPickerIntent,
    intent_request: Rc<dyn Fn(ProjectPickerIntent, &mut gpui::Window, &mut gpui::App)>,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .text_size(typography::ui())
        .text_color(color::current().gray.s09)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            intent_request(intent.clone(), window, cx);
        })
        .child(div().truncate().child(label))
        .child(
            div()
                .flex_shrink_0()
                .text_color(color::current().gray.s08)
                .child(hint),
        )
}
