//! 项目选择器 surface 的 L3 组件。
//!
//! 选择器是纯键盘 launcher：搜索、移动、打开、移除和克隆都由键盘驱动。

mod recent_projects;
mod search_box;
mod source_actions;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    Context, Corner, Div, Entity, FocusHandle, Keystroke, Window, div, point, prelude::*, px,
};

use crate::app::App;
use crate::focus::{AppFocus, ProjectPickerFocus};
use crate::shell::editor::TextEditorSlot;
use crate::shell::shared::theme::{color, radius, space};
use crate::shell::surfaces::{
    SurfaceAnchor, SurfaceId, SurfaceInvokerPoint, SurfaceManager, SurfacePlacement, SurfaceRequest,
};
use crate::shell::{KeyRequest, normalized_chord};

use super::recent::{RecentProject, RecentProjects};
use super::{ProjectPickerActions, ProjectPickerMode};

#[derive(Clone)]
pub(crate) struct ProjectPickerRuntime {
    focus: FocusHandle,
    key_request: Rc<RefCell<Option<KeyRequest>>>,
    slot: Rc<RefCell<Option<Rc<TextEditorSlot>>>>,
    /// 最近项目列表 —— picker 自家的纯 UI 数据，磁盘持久化由 [`RecentProjects`] 内部完成。
    /// 用 `Rc<RefCell>` 让 runtime 自身 `Clone` 时所有副本共享同一份。
    recent: Rc<RefCell<RecentProjects>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectPickerInitialMode {
    Browse,
    CloneGit,
}

impl ProjectPickerRuntime {
    /// 构造一个 picker runtime；`recent_path` 决定最近项目持久化位置，
    /// `None` 走纯内存模式（单测 / playground）。
    pub(crate) fn new<T>(cx: &mut gpui::Context<T>, recent_path: Option<PathBuf>) -> Self {
        Self {
            focus: cx.focus_handle(),
            key_request: Rc::new(RefCell::new(None)),
            slot: Rc::new(RefCell::new(None)),
            recent: Rc::new(RefCell::new(RecentProjects::load(recent_path))),
        }
    }

    /// 当前最近项目快照（克隆）。回调里用：调用方不持有 RefCell 借用。
    pub(crate) fn recent_projects(&self) -> Vec<RecentProject> {
        self.recent.borrow().items().to_vec()
    }

    /// 项目打开成功后由 shell 侧调一次，把它记进"最近"。
    /// 落盘在 [`RecentProjects::remember`] 内完成，调用方无需再 flush。
    pub(crate) fn remember_project(&self, root: PathBuf, repo: Option<String>) {
        self.recent.borrow_mut().remember(root, repo);
    }

    /// 从最近列表移除并把 picker 的当前 selection clamp 回合法区间。
    /// clamp 需要可变 picker；callee 在外部 borrow_mut。
    pub(crate) fn remove_recent(&self, id: &str) {
        self.recent.borrow_mut().remove(id);
    }

    pub(crate) fn set_key_request(&self, key_request: KeyRequest) {
        *self.key_request.borrow_mut() = Some(key_request);
    }

    pub(crate) fn set_slot(&self, slot: Rc<TextEditorSlot>) {
        *self.slot.borrow_mut() = Some(slot);
    }

    pub(crate) fn slot(&self) -> Option<Rc<TextEditorSlot>> {
        self.slot.borrow().clone()
    }

    pub(crate) fn install_listeners<T: 'static>(
        &self,
        app: Rc<RefCell<App>>,
        surfaces: Entity<SurfaceManager>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let focus = self.focus.clone();
        let focus_on_focus = focus.clone();
        let app_on_focus = Rc::clone(&app);
        cx.on_focus(&focus_on_focus, window, move |_, _, cx| {
            app_on_focus
                .borrow_mut()
                .request_focus_from_shell(AppFocus::project_picker(ProjectPickerFocus::Query));
            cx.notify();
        })
        .detach();
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
    let key_request = Rc::clone(&runtime.key_request);
    SurfaceRequest {
        id: SurfaceId::ProjectPicker,
        anchor: SurfaceAnchor::Invoker(super::INVOKER_ID.into()),
        placement: SurfacePlacement {
            invoker_point: SurfaceInvokerPoint::BottomLeft,
            corner: Corner::TopLeft,
            offset: point(px(0.0), space::s4()),
            fallback_position: point(px(48.0), px(28.0)),
        },
        focus_on_open: Some(focus),
        render: Rc::new(move || {
            render(runtime.clone(), actions.clone(), Rc::clone(&key_request)).into_any_element()
        }),
    }
}

fn render(
    runtime: ProjectPickerRuntime,
    actions: ProjectPickerActions,
    key_request: Rc<RefCell<Option<KeyRequest>>>,
) -> Div {
    let projects = (actions.projects)();
    let state = (actions.state)();
    let query_text = state
        .query
        .lines
        .first()
        .map(|line| line.text.as_str())
        .unwrap_or("");
    let visible = super::filtered_projects(&projects, query_text);
    let key_request_for_handler = Rc::clone(&key_request);

    let project_list = recent_projects::render(
        &visible,
        state.selected,
        state.mode,
        query_text.is_empty(),
        &actions,
    );

    div()
        .w(px(420.0))
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::s05())
        .bg(color::gray::s03())
        .overflow_hidden()
        .track_focus(&runtime.focus)
        .tab_index(0)
        .on_key_down(move |event, window, cx| {
            handle_key(
                Rc::clone(&key_request_for_handler),
                &event.keystroke,
                window,
                cx,
            );
        })
        .child(search_box::render(&state, &actions.slot))
        .child(project_list)
        .child(source_actions::render(state.mode, &actions))
}

fn handle_key(
    key_request: Rc<RefCell<Option<KeyRequest>>>,
    keystroke: &Keystroke,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    let Some(key_request) = key_request.borrow().clone() else {
        return;
    };
    let chord = normalized_chord(keystroke);
    if key_request(chord, window, cx) {
        cx.stop_propagation();
    }
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
