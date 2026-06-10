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
use crate::editor::TextEditorSlot;
use crate::focus::AppFocus;
use crate::shell::surfaces::{SurfaceAnchor, SurfaceManager, SurfaceRequest};
use crate::shell::{KeyRequest, normalized_chord};
use crate::text_target::TextTargetOwner;
use crate::theme::{color, radius};
use crate::ui_id::SurfaceId;

use super::recent::{RecentProject, RecentProjects};
use super::{
    ProjectPickerActions, ProjectPickerActivation, ProjectPickerMode, ProjectPickerModel,
    ProjectPickerState,
};

#[derive(Clone)]
pub(crate) struct ProjectPickerRuntime {
    focus: FocusHandle,
    key_request: Rc<RefCell<Option<KeyRequest>>>,
    slot: Rc<RefCell<Option<Rc<TextEditorSlot>>>>,
    /// 项目选择器 model 的真正拥有者。App 只通过 `owner_handle` 把它接入
    /// editor router；状态 / selection / activation 由 runtime 薄代理。
    model: Rc<RefCell<ProjectPickerModel>>,
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
            model: Rc::new(RefCell::new(ProjectPickerModel::new())),
            recent: Rc::new(RefCell::new(RecentProjects::load(recent_path))),
        }
    }

    /// 把 picker query owner 注册给 App 的路由表；`Rc` clone 不复制内部状态。
    pub(crate) fn owner_handle(&self) -> Rc<RefCell<dyn TextTargetOwner>> {
        self.model.clone()
    }

    pub(crate) fn reset(&self, mode: ProjectPickerMode) {
        self.model.borrow_mut().reset(mode);
    }

    pub(crate) fn state(&self) -> ProjectPickerState {
        self.model.borrow().state()
    }

    pub(crate) fn selected_project_id(&self, projects: &[RecentProject]) -> Option<String> {
        self.model.borrow().selected_project_id(projects)
    }

    pub(crate) fn move_selection(&self, delta: isize, projects: &[RecentProject]) {
        self.model.borrow_mut().move_selection(delta, projects);
    }

    pub(crate) fn clamp_selection(&self, projects: &[RecentProject]) {
        self.model.borrow_mut().clamp_selection(projects);
    }

    pub(crate) fn activation(&self, projects: &[RecentProject]) -> ProjectPickerActivation {
        self.model.borrow().activation(projects)
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

    /// 从最近列表移除；调用方随后让 picker model clamp 当前 selection。
    pub(crate) fn remove_recent(&self, id: &str) {
        self.recent.borrow_mut().remove(id);
    }

    /// 取走最近项目持久化层累积的人类可读警告。
    pub(crate) fn take_recent_warnings(&self) -> Vec<String> {
        self.recent.borrow_mut().take_warnings()
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
                .request_focus_from_shell(AppFocus::project_picker());
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
) -> SurfaceRequest {
    let focus = runtime.focus.clone();
    let key_request = Rc::clone(&runtime.key_request);
    SurfaceRequest {
        id: SurfaceId::ProjectPicker,
        anchor: SurfaceAnchor::Invoker {
            id: super::INVOKER_ID.into(),
            attachment: Corner::TopLeft,
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
