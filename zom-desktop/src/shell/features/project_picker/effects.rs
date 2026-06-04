//! 项目选择器 HostEffect 落地。
//!
//! view 把 HostEffect 流过来，本模块只认 picker 相关的 6 个变体；其余
//! 一律返回 `false`。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, FocusHandle, Window};
use zom_command::HostEffect;

use crate::app::App;
use crate::shell::features::panels::file_tree::FileTreeRuntime;
use crate::shell::features::project_picker::{
    self, ProjectPickerActions, ProjectPickerActivation, ProjectPickerInitialMode,
    ProjectPickerRuntime,
};
use crate::shell::surfaces::{SurfaceId, SurfaceManager};
use crate::shell::view::actions::open_surface;
use crate::shell::view::project;
use crate::shell::workbench::controller::WorkbenchController;

pub(crate) fn try_apply_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    file_tree_runtime: &FileTreeRuntime,
    project_picker_runtime: &ProjectPickerRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) -> bool {
    match effect {
        HostEffect::ShowProjectPicker => {
            show_project_picker(
                ProjectPickerInitialMode::Browse,
                app,
                surfaces,
                editor_focus_fallback,
                project_picker_runtime,
                window,
                cx,
            );
        }
        HostEffect::OpenLocalProject => {
            if surfaces.read_with(cx, |manager, _| manager.is_active(SurfaceId::ProjectPicker)) {
                app.borrow_mut().project_picker_deactivate();
            }
            project::open_local_project(
                Rc::clone(app),
                Rc::clone(workbench),
                surfaces,
                file_tree_runtime.clone(),
                project_picker_runtime.clone(),
                window,
                cx,
            );
        }
        HostEffect::StartGitClone => {
            show_project_picker(
                ProjectPickerInitialMode::CloneGit,
                app,
                surfaces,
                editor_focus_fallback,
                project_picker_runtime,
                window,
                cx,
            );
        }
        HostEffect::RemoveSelectedRecentProject => {
            if !picker_active(surfaces, cx) {
                return true;
            }
            let recent = project_picker_runtime.recent_projects();
            let project_id = project_picker_runtime.selected_project_id(&recent);
            if let Some(project_id) = project_id {
                project_picker_runtime.remove_recent(&project_id);
                let recent = project_picker_runtime.recent_projects();
                project_picker_runtime.clamp_selection(&recent);
                window.refresh();
            }
        }
        HostEffect::ProjectPickerMoveSelection(delta) => {
            if !picker_active(surfaces, cx) {
                return true;
            }
            let delta = *delta;
            let recent = project_picker_runtime.recent_projects();
            project_picker_runtime.move_selection(delta, &recent);
            window.refresh();
        }
        HostEffect::ProjectPickerActivate => {
            if !picker_active(surfaces, cx) {
                return true;
            }
            let recent = project_picker_runtime.recent_projects();
            let activation = project_picker_runtime.activation(&recent);
            match activation {
                ProjectPickerActivation::None => {}
                ProjectPickerActivation::Open(project_record) => {
                    app.borrow_mut().project_picker_deactivate();
                    project::open_recent_project(
                        Rc::clone(app),
                        Rc::clone(workbench),
                        surfaces,
                        file_tree_runtime.clone(),
                        project_picker_runtime.clone(),
                        project_record.path,
                        project_record.repo,
                        window,
                        cx,
                    );
                }
                ProjectPickerActivation::CloneGit(repo) => {
                    app.borrow_mut().project_picker_deactivate();
                    project::clone_git_project(
                        Rc::clone(app),
                        Rc::clone(workbench),
                        surfaces,
                        file_tree_runtime.clone(),
                        project_picker_runtime.clone(),
                        repo,
                        window,
                        cx,
                    );
                }
            }
        }
        _ => return false,
    }
    true
}

fn picker_active(surfaces: &Entity<SurfaceManager>, cx: &mut gpui::App) -> bool {
    surfaces.read_with(cx, |manager, _| manager.is_active(SurfaceId::ProjectPicker))
}

fn show_project_picker(
    initial_mode: ProjectPickerInitialMode,
    app: &Rc<RefCell<App>>,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    project_picker_runtime: &ProjectPickerRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let Some(project_picker_slot) = project_picker_runtime.slot() else {
        eprintln!("项目选择器未安装 TextEditorSlot");
        return;
    };
    project_picker_runtime.reset(initial_mode.into());
    app.borrow_mut()
        .request_focus(crate::focus::AppFocus::project_picker(
            crate::focus::ProjectPickerFocus::Query,
        ));
    let runtime_for_projects = project_picker_runtime.clone();
    let projects = Rc::new(move || runtime_for_projects.recent_projects());
    let state_runtime = project_picker_runtime.clone();
    let state = Rc::new(move || state_runtime.state());
    let shortcut_app = Rc::clone(app);
    let shortcut_lookup =
        Rc::new(move |command_id: &str| shortcut_app.borrow().shortcut_for(command_id));
    let title_app = Rc::clone(app);
    let command_title_lookup =
        Rc::new(move |command_id: &str| title_app.borrow().command_title_for(command_id));
    let actions = ProjectPickerActions {
        projects,
        state,
        slot: project_picker_slot,
        shortcut_lookup,
        command_title_lookup,
    };
    open_surface(
        project_picker::request(project_picker_runtime.clone(), actions, initial_mode),
        surfaces,
        editor_focus_fallback,
        window,
        cx,
    );
}
