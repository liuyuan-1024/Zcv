use gpui::{Div, div, prelude::*};
use zom_command::commands::project_picker as project_picker_commands;

use crate::shell::features::project_picker::RecentProject;
use crate::shell::shared::Glyph;
use crate::shell::shared::theme::{color, radius, space, typography};

use super::{ProjectPickerActions, ProjectPickerMode, command_title};

pub(super) fn render(
    projects: &[RecentProject],
    selected: usize,
    mode: ProjectPickerMode,
    query_is_empty: bool,
    actions: &ProjectPickerActions,
) -> Div {
    let mut list = div()
        .flex()
        .flex_col()
        .p(space::s6())
        .border_b_1()
        .border_color(color::gray::s05());
    if mode == ProjectPickerMode::CloneGit {
        return list.child(clone_hint());
    }
    if projects.is_empty() {
        let message = if query_is_empty {
            "暂无最近项目"
        } else {
            "没有匹配的项目"
        };
        return list.child(empty_hint(message));
    }

    for (index, project) in projects.iter().enumerate() {
        list = list.child(project_row(project, index, index == selected, actions));
    }
    list
}

fn project_row(
    project: &RecentProject,
    index: usize,
    selected: bool,
    actions: &ProjectPickerActions,
) -> Div {
    let border = if selected {
        color::blue::s07()
    } else {
        gpui::rgba(0)
    };
    let bg = if selected {
        color::gray::s04()
    } else {
        gpui::rgba(0)
    };
    let text_color = if selected {
        color::gray::s09()
    } else {
        color::gray::s09()
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .rounded(radius::r2())
        .border_1()
        .border_color(border)
        .bg(bg)
        .overflow_hidden()
        .text_size(typography::ui())
        .text_color(text_color)
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .truncate()
                .child(project.name.clone()),
        )
        .child(project_actions(index, actions))
}

fn project_actions(index: usize, actions: &ProjectPickerActions) -> Div {
    let remove_command = project_picker_commands::REMOVE_RECENT_PROJECT;
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .size(typography::ui_line())
        .child(
            Glyph::icon(
                ("project-picker.remove-recent", index),
                "icons/actions/close.svg",
                command_title(actions, remove_command),
            )
            .hint((actions.shortcut_lookup)(remove_command))
            .render(),
        )
}

fn empty_hint(message: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .text_size(typography::ui())
        .text_color(color::gray::s08())
        .child(message)
}

fn clone_hint() -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .text_size(typography::ui())
        .text_color(color::gray::s08())
        .child("回车后选择克隆位置")
}
