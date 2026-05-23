//! 项目选择器 surface 的 L3 组件。
//!
//! 选择器是纯键盘 launcher：搜索、移动、打开、移除和克隆都由键盘驱动。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Corner, Div, FocusHandle, Keystroke, div, point, prelude::*, px};
use zom_command::commands::workspace as workspace_commands;

use crate::app::RecentProject;
use crate::shell::normalized_chord;
use crate::shell::shared::Glyph;
use crate::shell::shared::theme::{color, radius, space, typography};
use crate::shell::surfaces::{
    SurfaceAnchor, SurfaceId, SurfaceInvokerPoint, SurfacePlacement, SurfaceRequest,
};

use super::ProjectPickerActions;

const REMOVE_ICON: &str = "icons/features/tab/close.svg";

#[derive(Clone)]
pub(crate) struct ProjectPickerRuntime {
    focus: FocusHandle,
    state: Rc<RefCell<ProjectPickerState>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerMode {
    Browse,
    CloneGit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectPickerInitialMode {
    Browse,
    CloneGit,
}

#[derive(Clone, Debug)]
struct ProjectPickerState {
    query: String,
    selected: usize,
    mode: PickerMode,
}

impl ProjectPickerRuntime {
    pub(crate) fn new<T>(cx: &mut gpui::Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
            state: Rc::new(RefCell::new(ProjectPickerState {
                query: String::new(),
                selected: 0,
                mode: PickerMode::Browse,
            })),
        }
    }

    fn reset(&self, mode: ProjectPickerInitialMode) {
        *self.state.borrow_mut() = ProjectPickerState {
            query: String::new(),
            selected: 0,
            mode: mode.into(),
        };
    }

    pub(crate) fn selected_project_id(&self, projects: &[RecentProject]) -> Option<String> {
        let state = self.state.borrow().clone();
        if state.mode != PickerMode::Browse {
            return None;
        }
        filtered_projects(projects, &state.query)
            .get(state.selected)
            .map(|project| project.id.clone())
    }
}

pub(crate) fn request(
    runtime: ProjectPickerRuntime,
    actions: ProjectPickerActions,
    initial_mode: ProjectPickerInitialMode,
) -> SurfaceRequest {
    runtime.reset(initial_mode);
    let focus = runtime.focus.clone();
    SurfaceRequest {
        id: SurfaceId::ProjectPicker,
        anchor: SurfaceAnchor::Invoker(super::INVOKER_ID.into()),
        placement: SurfacePlacement {
            invoker_point: SurfaceInvokerPoint::BottomLeft,
            corner: Corner::TopLeft,
            offset: point(px(0.0), space::s8()),
            fallback_position: point(px(48.0), px(28.0)),
        },
        focus_on_open: Some(focus),
        render: Rc::new(move || render(runtime.clone(), actions.clone()).into_any_element()),
    }
}

fn render(runtime: ProjectPickerRuntime, actions: ProjectPickerActions) -> Div {
    let projects = (actions.projects)();
    let state = runtime.state.borrow().clone();
    let visible = filtered_projects(&projects, &state.query);
    clamp_selection(&runtime.state, visible.len());
    let state = runtime.state.borrow().clone();
    let key_runtime = runtime.clone();
    let key_actions = actions.clone();

    let mut list = div().flex().flex_col().gap(space::s4());
    if state.mode == PickerMode::CloneGit {
        list = list.child(clone_hint());
    } else if visible.is_empty() {
        list = list.child(empty_hint(if state.query.is_empty() {
            "暂无最近项目"
        } else {
            "没有匹配的项目"
        }));
    } else {
        for (index, project) in visible.iter().enumerate() {
            list = list.child(project_row(
                project,
                index,
                index == state.selected,
                &actions,
            ));
        }
    }

    div()
        .w(px(420.0))
        .p(space::s12())
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::g40())
        .bg(color::gray::g10())
        .track_focus(&runtime.focus)
        .tab_index(0)
        .on_key_down(move |event, window, cx| {
            handle_key(&key_runtime, &key_actions, &event.keystroke, window, cx);
            cx.stop_propagation();
        })
        .child(first_section(input_box(&state)))
        .child(section(list))
        .child(section(action_section(state.mode, &actions)))
}

fn handle_key(
    runtime: &ProjectPickerRuntime,
    actions: &ProjectPickerActions,
    keystroke: &Keystroke,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    let chord = normalized_chord(keystroke);
    if (actions.key_request)(chord.clone(), window, cx) {
        return;
    }

    match chord.as_str() {
        "up" => {
            let count = visible_count(actions, runtime);
            runtime.state.borrow_mut().move_selection(-1, count);
            window.refresh();
        }
        "down" => {
            let count = visible_count(actions, runtime);
            runtime.state.borrow_mut().move_selection(1, count);
            window.refresh();
        }
        "enter" | "return" => {
            activate_selection(runtime, actions, window, cx);
        }
        "backspace" => {
            runtime.state.borrow_mut().query.pop();
            window.refresh();
        }
        _ => {
            if let Some(text) = typed_text(keystroke) {
                runtime.state.borrow_mut().push_text(&text);
                window.refresh();
            }
        }
    }
}

fn activate_selection(
    runtime: &ProjectPickerRuntime,
    actions: &ProjectPickerActions,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    let state = runtime.state.borrow().clone();
    if state.mode == PickerMode::CloneGit {
        let repo = state.query.trim();
        if !repo.is_empty() {
            (actions.clone_git_project)(repo.to_string(), window, cx);
        }
        return;
    }

    let projects = (actions.projects)();
    let visible = filtered_projects(&projects, &state.query);
    let Some(project) = visible.get(state.selected) else {
        return;
    };
    (actions.open_project)(project.clone(), window, cx);
}

fn visible_count(actions: &ProjectPickerActions, runtime: &ProjectPickerRuntime) -> usize {
    let projects = (actions.projects)();
    let state = runtime.state.borrow();
    filtered_projects(&projects, &state.query).len()
}

fn typed_text(keystroke: &Keystroke) -> Option<String> {
    if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.function {
        return None;
    }
    keystroke
        .key_char
        .as_ref()
        .filter(|text| !text.is_empty())
        .cloned()
}

fn filtered_projects(projects: &[RecentProject], query: &str) -> Vec<RecentProject> {
    let terms = query
        .split_whitespace()
        .map(|term| term.to_lowercase())
        .collect::<Vec<_>>();
    if terms.is_empty() {
        return projects.to_vec();
    }

    projects
        .iter()
        .filter(|project| {
            let haystack = format!(
                "{} {} {}",
                project.name,
                project.identifier,
                project.path.display()
            )
            .to_lowercase();
            terms.iter().all(|term| haystack.contains(term))
        })
        .cloned()
        .collect()
}

fn clamp_selection(state: &Rc<RefCell<ProjectPickerState>>, count: usize) {
    let mut state = state.borrow_mut();
    if count == 0 {
        state.selected = 0;
    } else if state.selected >= count {
        state.selected = count - 1;
    }
}

fn input_box(state: &ProjectPickerState) -> Div {
    let placeholder = match state.mode {
        PickerMode::Browse => "搜索项目名或路径",
        PickerMode::CloneGit => "输入 Git 仓库地址",
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
        .h(px(32.0))
        .flex()
        .items_center()
        .px(space::s8())
        .rounded(radius::r4())
        .border_1()
        .border_color(color::focus::border())
        .bg(color::gray::g05())
        .text_size(typography::ui())
        .text_color(text_color)
        .overflow_hidden()
        .child(div().truncate().child(text))
}

fn project_row(
    project: &RecentProject,
    index: usize,
    selected: bool,
    actions: &ProjectPickerActions,
) -> Div {
    let border = if selected {
        color::focus::border()
    } else {
        gpui::rgba(0)
    };
    let bg = if selected {
        color::gray::g20()
    } else {
        gpui::rgba(0)
    };
    div()
        .h(px(44.0))
        .flex()
        .items_center()
        .gap(space::s8())
        .px(space::s8())
        .rounded(radius::r4())
        .border_1()
        .border_color(border)
        .bg(bg)
        .overflow_hidden()
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .child(
                    div()
                        .text_size(typography::ui())
                        .text_color(color::gray::g95())
                        .truncate()
                        .child(project.name.clone()),
                )
                .child(
                    div()
                        .mt(space::s2())
                        .text_size(typography::ui())
                        .text_color(color::gray::g60())
                        .truncate()
                        .child(project.identifier.clone()),
                ),
        )
        .child(project_actions(index, actions))
}

fn project_actions(index: usize, actions: &ProjectPickerActions) -> Div {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .size(typography::ui_line())
        .child(
            Glyph::icon(
                ("project-picker.remove-recent", index),
                REMOVE_ICON,
                "移除最近项目记录",
            )
            .hint((actions.shortcut_lookup)(
                workspace_commands::REMOVE_RECENT_PROJECT,
            ))
            .render(),
        )
}

fn empty_hint(message: &'static str) -> Div {
    div()
        .h(px(64.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(typography::ui())
        .text_color(color::gray::g60())
        .child(message)
}

fn clone_hint() -> Div {
    div()
        .h(px(64.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(typography::ui())
        .text_color(color::gray::g60())
        .child("回车后选择克隆位置")
}

fn section(child: Div) -> Div {
    div()
        .py(space::s8())
        .border_t_1()
        .border_color(color::gray::g40())
        .child(child)
}

fn first_section(child: Div) -> Div {
    div().pb(space::s8()).child(child)
}

fn action_section(mode: PickerMode, actions: &ProjectPickerActions) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::s4())
        .child(action_row(
            "从本地路径导入",
            command_shortcut(
                actions,
                workspace_commands::OPEN_LOCAL_PROJECT,
                "Cmd/Ctrl+L",
            ),
        ))
        .child(action_row(
            "从 Git 地址克隆",
            if mode == PickerMode::CloneGit {
                "Enter".to_string()
            } else {
                command_shortcut(actions, workspace_commands::START_GIT_CLONE, "Cmd/Ctrl+G")
            },
        ))
}

fn command_shortcut(
    actions: &ProjectPickerActions,
    command_id: &'static str,
    fallback: &'static str,
) -> String {
    (actions.shortcut_lookup)(command_id).unwrap_or_else(|| fallback.to_string())
}

fn action_row(label: &'static str, hint: String) -> Div {
    div()
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_between()
        .px(space::s8())
        .rounded(radius::r4())
        .text_size(typography::ui())
        .text_color(color::gray::g75())
        .child(div().truncate().child(label))
        .child(
            div()
                .flex_shrink_0()
                .text_color(color::gray::g60())
                .child(hint),
        )
}

impl From<ProjectPickerInitialMode> for PickerMode {
    fn from(mode: ProjectPickerInitialMode) -> Self {
        match mode {
            ProjectPickerInitialMode::Browse => Self::Browse,
            ProjectPickerInitialMode::CloneGit => Self::CloneGit,
        }
    }
}

impl ProjectPickerState {
    fn move_selection(&mut self, delta: isize, count: usize) {
        if count == 0 {
            self.selected = 0;
            return;
        }
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, count as isize - 1) as usize;
    }

    fn push_text(&mut self, text: &str) {
        self.query.push_str(text);
        self.selected = 0;
    }
}
