use std::rc::Rc;

use gpui::{Div, MouseButton, div, prelude::*};

use crate::shell::features::project_picker::{ProjectPickerIntent, RecentProject};
use crate::shell::shared::{CommandBinding, Glyph};
use crate::theme::{color, radius, space, typography};

use super::{ProjectPickerActions, ProjectPickerMode};

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
        .border_color(color::current().gray.s05);
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
        color::current().blue.s07
    } else {
        gpui::rgba(0)
    };
    let select = Rc::clone(&actions.select);
    let intent_request = Rc::clone(&actions.intent_request);
    div()
        .flex()
        .flex_row()
        .items_center()
        .rounded(radius::r2())
        .border_1()
        .border_color(border)
        .hover(|style| style.bg(color::current().gray.s04))
        .overflow_hidden()
        .text_color(color::current().gray.s09)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            (select)(index);
            intent_request(ProjectPickerIntent::Activate, window, cx);
        })
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .truncate()
                .child(project.name.clone()),
        )
        .child(project_actions(project, index, actions))
}

fn project_actions(project: &RecentProject, index: usize, actions: &ProjectPickerActions) -> Div {
    let project_id = project.id.clone();
    let intent_request = Rc::clone(&actions.intent_request);

    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .size(typography::ui_line())
        .child(
            Glyph::icon(
                ("project-picker.remove-recent", index),
                "icons/actions/trash.svg",
            )
            .command(CommandBinding {
                request: Rc::new(move |window, cx| {
                    intent_request(
                        ProjectPickerIntent::RemoveRecentProject {
                            id: project_id.clone(),
                        },
                        window,
                        cx,
                    );
                }),
                ..actions.remove_recent_command.clone()
            })
            .render(),
        )
}

fn empty_hint(message: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color::current().gray.s08)
        .child(message)
}

fn clone_hint() -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color::current().gray.s08)
        .child("回车后选择克隆位置")
}
