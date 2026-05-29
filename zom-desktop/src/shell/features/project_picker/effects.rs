//! 项目选择器 HostEffect 落地。
//!
//! view 把 HostEffect 流过来，本模块只认 picker 相关的 6 个变体；其余
//! 一律返回 `false`。
//!
//! `show_project_picker` 里挂的 key_request 闭包要把按键再喂回宿主的
//! effect 管线，故本模块反向 use 了 `view::actions::apply_host_effects`
//! —— 这条 feature→view 反向依赖是 picker re-entry 的固有耦合。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, FocusHandle, Window};
use zom_command::HostEffect;

use crate::app::App;
use crate::shell::editor::TextEditorSlot;
use crate::shell::features::language_servers::LanguageServersRuntime;
use crate::shell::features::panels::PanelRuntimes;
use crate::shell::features::panels::file_tree::FileTreeRuntime;
use crate::shell::features::project_picker::{
    self, ProjectPickerActions, ProjectPickerActivation, ProjectPickerInitialMode,
    ProjectPickerRuntime,
};
use crate::shell::platform::clipboard::GpuiClipboardScope;
use crate::shell::surfaces::{SurfaceId, SurfaceManager};
use crate::shell::view::actions::{apply_host_effects, open_surface};
use crate::shell::view::project;
use crate::shell::workbench::controller::WorkbenchController;

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_apply_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    panel_runtimes: &PanelRuntimes,
    file_tree: &FileTreeRuntime,
    project_picker_runtime: &ProjectPickerRuntime,
    language_servers_runtime: &LanguageServersRuntime,
    project_picker_slot: &Rc<TextEditorSlot>,
    window: &mut Window,
    cx: &mut gpui::App,
) -> bool {
    match effect {
        HostEffect::ShowProjectPicker => {
            show_project_picker(
                ProjectPickerInitialMode::Browse,
                app,
                workbench,
                surfaces,
                editor_focus_fallback,
                panel_runtimes,
                file_tree,
                project_picker_runtime,
                language_servers_runtime,
                project_picker_slot,
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
                file_tree.clone(),
                window,
                cx,
            );
        }
        HostEffect::StartGitClone => {
            show_project_picker(
                ProjectPickerInitialMode::CloneGit,
                app,
                workbench,
                surfaces,
                editor_focus_fallback,
                panel_runtimes,
                file_tree,
                project_picker_runtime,
                language_servers_runtime,
                project_picker_slot,
                window,
                cx,
            );
        }
        HostEffect::RemoveSelectedRecentProject => {
            if !picker_active(surfaces, cx) {
                return true;
            }
            let project_id = app
                .borrow()
                .with_project_picker_ref(|picker, recent| picker.selected_project_id(recent));
            if let Some(project_id) = project_id {
                app.borrow_mut().remove_recent_project(&project_id);
                window.refresh();
            }
        }
        HostEffect::ProjectPickerMoveSelection(delta) => {
            if !picker_active(surfaces, cx) {
                return true;
            }
            let delta = *delta;
            app.borrow_mut()
                .with_project_picker(|picker, recent| picker.move_selection(delta, recent));
            window.refresh();
        }
        HostEffect::ProjectPickerActivate => {
            if !picker_active(surfaces, cx) {
                return true;
            }
            let activation = app
                .borrow()
                .with_project_picker_ref(|picker, recent| picker.activation(recent));
            match activation {
                ProjectPickerActivation::None => {}
                ProjectPickerActivation::Open(project_record) => {
                    app.borrow_mut().project_picker_deactivate();
                    project::open_recent_project(
                        Rc::clone(app),
                        Rc::clone(workbench),
                        surfaces,
                        file_tree.clone(),
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
                        file_tree.clone(),
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

#[allow(clippy::too_many_arguments)]
fn show_project_picker(
    initial_mode: ProjectPickerInitialMode,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    panel_runtimes: &PanelRuntimes,
    file_tree: &FileTreeRuntime,
    project_picker_runtime: &ProjectPickerRuntime,
    language_servers_runtime: &LanguageServersRuntime,
    project_picker_slot: &Rc<TextEditorSlot>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    app.borrow_mut().project_picker_reset(initial_mode.into());
    let project_list_app = Rc::clone(app);
    let projects = Rc::new(move || project_list_app.borrow().recent_projects());
    let state_app = Rc::clone(app);
    let state = Rc::new(move || state_app.borrow().project_picker().state());
    let key_app = Rc::clone(app);
    let key_workbench = Rc::clone(workbench);
    let key_surfaces = surfaces.clone();
    let key_editor_focus = editor_focus_fallback.clone();
    let key_panel_runtimes = panel_runtimes.clone();
    let key_file_tree = file_tree.clone();
    let key_project_picker = project_picker_runtime.clone();
    let key_language_servers = language_servers_runtime.clone();
    let key_project_picker_slot = Rc::clone(project_picker_slot);
    let key_request = Rc::new(
        move |chord: String, window: &mut Window, cx: &mut gpui::App| {
            let outcome = {
                let _clip = GpuiClipboardScope::enter(cx);
                match key_app.borrow_mut().dispatch_key(chord) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        eprintln!("命令执行失败：{error}");
                        return false;
                    }
                }
            };

            apply_host_effects(
                outcome.effects,
                &key_app,
                &key_workbench,
                &key_surfaces,
                &key_editor_focus,
                &key_panel_runtimes,
                &key_file_tree,
                &key_project_picker,
                &key_language_servers,
                &key_project_picker_slot,
                window,
                cx,
            );
            if outcome.consumed {
                window.refresh();
            }
            outcome.consumed
        },
    );
    let shortcut_app = Rc::clone(app);
    let shortcut_lookup =
        Rc::new(move |command_id: &str| shortcut_app.borrow().shortcut_for(command_id));
    let title_app = Rc::clone(app);
    let command_title_lookup =
        Rc::new(move |command_id: &str| title_app.borrow().command_title_for(command_id));
    let actions = ProjectPickerActions {
        projects,
        state,
        key_request,
        slot: Rc::clone(project_picker_slot),
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
