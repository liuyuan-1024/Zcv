//! 项目选择器 HostEffect 落地。
//!
//! view 把 HostEffect 流过来，本模块只认 picker 相关的 6 个变体；其余一律返回 `false`。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, FocusHandle, Window};
use zom_command::{HostEffect, ProjectEffect};

use crate::app::App;
use crate::focus::AppFocus;
use crate::shell::bubble::BubbleRuntime;
use crate::shell::features::panels::file_tree::FileTreeRuntime;
use crate::shell::features::project_picker::{
    self, ProjectPickerActions, ProjectPickerActivation, ProjectPickerInitialMode,
    ProjectPickerIntent, ProjectPickerIntentRequest, ProjectPickerMode, ProjectPickerRuntime,
};
use crate::shell::surfaces::SurfaceManager;
use crate::shell::view::actions::open_surface;
use crate::shell::workbench::controller::WorkbenchController;
use crate::shell::{CommandTitleLookup, ShortcutLookup, project_session, shared};
use crate::ui_id::SurfaceId;
use zom_command::BubbleRequest;

pub(crate) fn try_apply_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    file_tree_runtime: &FileTreeRuntime,
    project_picker_runtime: &ProjectPickerRuntime,
    bubbles: &Entity<BubbleRuntime>,
    window: &mut Window,
    cx: &mut gpui::App,
) -> bool {
    match effect {
        HostEffect::Project(ProjectEffect::ShowPicker) => {
            let intent_request = project_picker_intent_request(
                project_picker_runtime.clone(),
                Rc::clone(app),
                Rc::clone(workbench),
                surfaces.clone(),
                file_tree_runtime.clone(),
                bubbles.clone(),
            );
            show_project_picker(
                ProjectPickerInitialMode::Browse,
                app,
                surfaces,
                editor_focus_fallback,
                project_picker_runtime,
                intent_request,
                window,
                cx,
            );
        }
        HostEffect::Project(ProjectEffect::OpenLocalProject) => {
            project_session::open_local_project(
                Rc::clone(app),
                Rc::clone(workbench),
                surfaces,
                file_tree_runtime.clone(),
                project_picker_runtime.clone(),
                bubbles.clone(),
                window,
                cx,
            );
        }
        HostEffect::Project(ProjectEffect::StartGitClone) => {
            if picker_active(surfaces, cx) {
                // 已在 picker 浮面里切模式：只重置模型 + 刷新视图。
                // 不再走 open_surface —— 否则 focus_to_restore 会被当前 picker 自己的句柄覆盖，
                // ESC 关闭后焦点恢复到已卸载的元素，窗口失焦。
                project_picker_runtime.reset(ProjectPickerMode::CloneGit);
                window.refresh();
            } else {
                let intent_request = project_picker_intent_request(
                    project_picker_runtime.clone(),
                    Rc::clone(app),
                    Rc::clone(workbench),
                    surfaces.clone(),
                    file_tree_runtime.clone(),
                    bubbles.clone(),
                );
                show_project_picker(
                    ProjectPickerInitialMode::CloneGit,
                    app,
                    surfaces,
                    editor_focus_fallback,
                    project_picker_runtime,
                    intent_request,
                    window,
                    cx,
                );
            }
        }
        HostEffect::Project(ProjectEffect::RemoveSelectedRecentProject) => {
            if !picker_active(surfaces, cx) {
                return true;
            }
            let recent = project_picker_runtime.recent_projects();
            let project_id = project_picker_runtime.selected_project_id(&recent);
            if let Some(project_id) = project_id {
                project_picker_runtime.remove_recent(&project_id);
                let recent = project_picker_runtime.recent_projects();
                project_picker_runtime.clamp_selection(&recent);
                for warning in project_picker_runtime.take_recent_warnings() {
                    bubbles.update(cx, |runtime, cx| {
                        runtime.push(BubbleRequest::error(warning).dedupe("project.recent"), cx);
                    });
                }
                window.refresh();
            }
        }
        HostEffect::Project(ProjectEffect::MovePickerSelection(delta)) => {
            if !picker_active(surfaces, cx) {
                return true;
            }
            let delta = *delta;
            let recent = project_picker_runtime.recent_projects();
            project_picker_runtime.move_selection(delta, &recent);
            window.refresh();
        }
        HostEffect::Project(ProjectEffect::ActivatePicker) => {
            if !picker_active(surfaces, cx) {
                return true;
            }
            let recent = project_picker_runtime.recent_projects();
            let activation = project_picker_runtime.activation(&recent);
            match activation {
                ProjectPickerActivation::None => {}
                ProjectPickerActivation::Open(project_record) => {
                    project_session::open_recent_project(
                        Rc::clone(app),
                        Rc::clone(workbench),
                        surfaces,
                        file_tree_runtime.clone(),
                        project_picker_runtime.clone(),
                        bubbles.clone(),
                        project_record.path,
                        project_record.repo,
                        window,
                        cx,
                    );
                }
                ProjectPickerActivation::CloneGit(repo) => {
                    project_session::clone_git_project(
                        Rc::clone(app),
                        Rc::clone(workbench),
                        surfaces,
                        file_tree_runtime.clone(),
                        project_picker_runtime.clone(),
                        bubbles.clone(),
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
    intent_request: ProjectPickerIntentRequest,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let Some(project_picker_slot) = project_picker_runtime.slot() else {
        eprintln!("项目选择器未安装 TextEditorSlot");
        return;
    };
    project_picker_runtime.reset(initial_mode.into());
    app.borrow_mut().request_focus(AppFocus::project_picker());
    let runtime_for_projects = project_picker_runtime.clone();
    let projects = Rc::new(move || runtime_for_projects.recent_projects());
    let state_runtime = project_picker_runtime.clone();
    let state = Rc::new(move || state_runtime.state());
    let shortcut_app = Rc::clone(app);
    let shortcut_lookup: ShortcutLookup =
        Rc::new(move |command_id: &str| shortcut_app.borrow().shortcuts_for(command_id));
    let title_app = Rc::clone(app);
    let command_title_lookup: CommandTitleLookup =
        Rc::new(move |command_id: &str| title_app.borrow().command_title_for(command_id));
    let remove_recent_command = shared::CommandBinding {
        id: zom_command::commands::project_picker::REMOVE_RECENT_PROJECT.to_string(),
        title: Rc::clone(&command_title_lookup),
        shortcut: Rc::clone(&shortcut_lookup),
        request: Rc::new(|_, _| {}),
    };
    let select = {
        let runtime = project_picker_runtime.clone();
        Rc::new(move |index| {
            let recent = runtime.recent_projects();
            runtime.select(index, &recent);
        })
    };
    let actions = ProjectPickerActions {
        projects,
        state,
        slot: project_picker_slot,
        intent_request,
        remove_recent_command,
        shortcut_lookup,
        command_title_lookup,
        select,
    };
    open_surface(
        project_picker::request(project_picker_runtime.clone(), actions),
        surfaces,
        editor_focus_fallback,
        window,
        cx,
    );
}

fn project_picker_intent_request(
    runtime: ProjectPickerRuntime,
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    surfaces: Entity<SurfaceManager>,
    file_tree: FileTreeRuntime,
    bubbles: Entity<BubbleRuntime>,
) -> ProjectPickerIntentRequest {
    Rc::new(move |intent, window, cx| match intent {
        ProjectPickerIntent::RemoveRecentProject { id } => {
            runtime.remove_recent(&id);
            let recent = runtime.recent_projects();
            runtime.clamp_selection(&recent);
            for warning in runtime.take_recent_warnings() {
                bubbles.update(cx, |runtime, cx| {
                    runtime.push(BubbleRequest::error(warning).dedupe("project.recent"), cx);
                });
            }
            window.refresh();
        }
        ProjectPickerIntent::Activate => {
            let recent = runtime.recent_projects();
            let activation = runtime.activation(&recent);
            match activation {
                ProjectPickerActivation::None => {}
                ProjectPickerActivation::Open(project_record) => {
                    project_session::open_recent_project(
                        Rc::clone(&app),
                        Rc::clone(&workbench),
                        &surfaces,
                        file_tree.clone(),
                        runtime.clone(),
                        bubbles.clone(),
                        project_record.path,
                        project_record.repo,
                        window,
                        cx,
                    );
                }
                ProjectPickerActivation::CloneGit(repo) => {
                    project_session::clone_git_project(
                        Rc::clone(&app),
                        Rc::clone(&workbench),
                        &surfaces,
                        file_tree.clone(),
                        runtime.clone(),
                        bubbles.clone(),
                        repo,
                        window,
                        cx,
                    );
                }
            }
        }
        ProjectPickerIntent::OpenLocalProject => {
            project_session::open_local_project(
                Rc::clone(&app),
                Rc::clone(&workbench),
                &surfaces,
                file_tree.clone(),
                runtime.clone(),
                bubbles.clone(),
                window,
                cx,
            );
        }
        ProjectPickerIntent::StartGitClone => {
            let is_active =
                surfaces.read_with(cx, |manager, _| manager.is_active(SurfaceId::ProjectPicker));
            if is_active {
                runtime.reset(ProjectPickerMode::CloneGit);
                window.refresh();
            }
        }
    })
}
