//! 项目选择器 surface 的 L3 组件。
//!
//! 选择器是纯键盘 launcher：搜索、移动、打开、移除和克隆都由键盘驱动。

mod recent_projects;
mod search_box;
mod source_actions;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Corner, Div, FocusHandle, Keystroke, div, point, prelude::*, px};

use crate::app::RecentProject;
use crate::shell::normalized_chord;
use crate::shell::shared::theme::{color, radius, space};
use crate::shell::surfaces::{
    SurfaceAnchor, SurfaceId, SurfaceInvokerPoint, SurfacePlacement, SurfaceRequest,
};

use super::ProjectPickerActions;

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

pub(crate) enum ProjectPickerActivation {
    None,
    Open(RecentProject),
    CloneGit(String),
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

    pub(crate) fn move_selection(&self, delta: isize, projects: &[RecentProject]) {
        let state = self.state.borrow().clone();
        if state.mode != PickerMode::Browse {
            return;
        }
        let count = filtered_projects(projects, &state.query).len();
        self.state.borrow_mut().move_selection(delta, count);
    }

    pub(crate) fn delete_query_char(&self) {
        self.state.borrow_mut().query.pop();
    }

    pub(crate) fn insert_query_text(&self, text: &str) {
        self.state.borrow_mut().push_text(text);
    }

    pub(crate) fn activation(&self, projects: &[RecentProject]) -> ProjectPickerActivation {
        let state = self.state.borrow().clone();
        if state.mode == PickerMode::CloneGit {
            let repo = state.query.trim();
            if repo.is_empty() {
                return ProjectPickerActivation::None;
            }
            return ProjectPickerActivation::CloneGit(repo.to_string());
        }

        filtered_projects(projects, &state.query)
            .get(state.selected)
            .cloned()
            .map(ProjectPickerActivation::Open)
            .unwrap_or(ProjectPickerActivation::None)
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
    let key_actions = actions.clone();

    let project_list = recent_projects::render(
        &visible,
        state.selected,
        state.mode,
        state.query.is_empty(),
        &actions,
    );

    div()
        .w(px(420.0))
        .p(space::s12())
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::g40())
        .bg(color::gray::g10())
        .overflow_hidden()
        .track_focus(&runtime.focus)
        .tab_index(0)
        .on_key_down(move |event, window, cx| {
            handle_key(&key_actions, &event.keystroke, window, cx);
            cx.stop_propagation();
        })
        .child(search_box::render(&state))
        .child(divided_section(project_list))
        .child(divided_section(source_actions::render(
            state.mode, &actions,
        )))
}

fn handle_key(
    actions: &ProjectPickerActions,
    keystroke: &Keystroke,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    let chord = normalized_chord(keystroke);
    if (actions.key_request)(chord.clone(), window, cx) {
        return;
    }

    if let Some(text) = typed_text(keystroke) {
        (actions.query_text_request)(text, window, cx);
    }
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

fn divided_section(child: Div) -> Div {
    div()
        .mt(space::s8())
        .pt(space::s8())
        .border_t_1()
        .border_color(color::gray::g40())
        .child(child)
}

pub(super) fn command_shortcut(actions: &ProjectPickerActions, command_id: &'static str) -> String {
    (actions.shortcut_lookup)(command_id).unwrap_or_default()
}

pub(super) fn command_title(actions: &ProjectPickerActions, command_id: &'static str) -> String {
    (actions.command_title_lookup)(command_id).unwrap_or_else(|| command_id.to_string())
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
