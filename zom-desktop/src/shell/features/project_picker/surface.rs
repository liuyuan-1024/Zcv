//! 项目选择器 surface 的 L3 组件。
//!
//! 选择器是纯键盘 launcher：搜索、移动、打开、移除和克隆都由键盘驱动。

mod recent_projects;
mod search_box;
mod source_actions;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Context, Corner, Div, Entity, FocusHandle, Keystroke, Window, div, point, prelude::*, px,
};

use crate::app::App;
use crate::shell::normalized_chord;
use crate::shell::shared::theme::{color, radius, space};
use crate::shell::surfaces::{
    SurfaceAnchor, SurfaceId, SurfaceInvokerPoint, SurfaceManager, SurfacePlacement, SurfaceRequest,
};

use super::{ProjectPickerActions, ProjectPickerMode};

#[derive(Clone)]
pub(crate) struct ProjectPickerRuntime {
    focus: FocusHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectPickerInitialMode {
    Browse,
    CloneGit,
}

impl ProjectPickerRuntime {
    pub(crate) fn new<T>(cx: &mut gpui::Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }

    pub(crate) fn install_listeners<T: 'static>(
        &self,
        app: Rc<RefCell<App>>,
        surfaces: Entity<SurfaceManager>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let focus = self.focus.clone();
        cx.on_blur(&focus, window, move |_, _, cx| {
            surfaces.update(cx, |surfaces, cx| {
                if surfaces.is_active(SurfaceId::ProjectPicker) {
                    app.borrow_mut().project_picker_deactivate();
                    surfaces.dismiss(cx);
                }
            });
            cx.notify();
        })
        .detach();
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }
}

pub(crate) fn request(
    runtime: ProjectPickerRuntime,
    actions: ProjectPickerActions,
    _initial_mode: ProjectPickerInitialMode,
) -> SurfaceRequest {
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
    let state = (actions.state)();
    let query_text = state.query.text();
    let visible = super::filtered_projects(&projects, &query_text);
    let key_actions = actions.clone();

    let project_list = recent_projects::render(
        &visible,
        state.selected,
        state.mode,
        query_text.is_empty(),
        &actions,
    );

    div()
        .w(px(420.0))
        .p(space::s8())
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::s05())
        .bg(color::gray::s03())
        .overflow_hidden()
        .track_focus(&runtime.focus)
        .tab_index(0)
        .on_key_down(move |event, window, cx| {
            handle_key(&key_actions, &event.keystroke, window, cx);
        })
        .child(search_box::render(&state, &actions.slot))
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
        cx.stop_propagation();
    }
}

fn divided_section(child: Div) -> Div {
    div()
        .mt(space::s8())
        .pt(space::s8())
        .border_t_1()
        .border_color(color::gray::s05())
        .child(child)
}

pub(super) fn command_shortcut(actions: &ProjectPickerActions, command_id: &'static str) -> String {
    (actions.shortcut_lookup)(command_id).unwrap_or_default()
}

pub(super) fn command_title(actions: &ProjectPickerActions, command_id: &'static str) -> String {
    (actions.command_title_lookup)(command_id).unwrap_or_else(|| command_id.to_string())
}

impl From<ProjectPickerInitialMode> for ProjectPickerMode {
    fn from(mode: ProjectPickerInitialMode) -> Self {
        match mode {
            ProjectPickerInitialMode::Browse => ProjectPickerMode::Browse,
            ProjectPickerInitialMode::CloneGit => ProjectPickerMode::CloneGit,
        }
    }
}
