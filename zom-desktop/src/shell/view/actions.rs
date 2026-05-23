//! ShellView 的命令动作与 HostEffect 解释。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, FocusHandle, Window};
use zom_command::{HostEffect, Invocation};

use crate::app::{App, KeySurface};
use crate::shell::ActionRequest;
use crate::shell::features::file_tree::{FileTreeActivation, FileTreeRuntime};
use crate::shell::features::project_picker::ProjectPickerRuntime;
use crate::shell::features::{PanelId, PanelRuntimes, language_servers, project_picker};
use crate::shell::platform::window as platform_window;
use crate::shell::surfaces::{SurfaceId, SurfaceManager, SurfaceRequest};
use crate::shell::workbench::controller::WorkbenchController;

use super::focus::{FocusRouter, FocusTarget};
use super::project;

pub(super) fn bind_action_request(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    surfaces: Entity<SurfaceManager>,
    editor_focus_fallback: FocusHandle,
    panel_runtimes: PanelRuntimes,
    file_tree: FileTreeRuntime,
    project_picker_runtime: ProjectPickerRuntime,
    invocation: Invocation,
) -> ActionRequest {
    Rc::new(move |window, cx| {
        let effects = match app.borrow_mut().dispatch(invocation.clone()) {
            Ok(effects) => effects,
            Err(error) => {
                eprintln!("命令执行失败：{error}");
                return;
            }
        };
        apply_host_effects(
            effects,
            &app,
            &workbench,
            &surfaces,
            &editor_focus_fallback,
            &panel_runtimes,
            &file_tree,
            &project_picker_runtime,
            window,
            cx,
        );
        // 命令可能改了渲染可见的模型状态（如关闭删除确认弹窗）；与 key_request
        // 的按键路径对称，点击路径在此统一刷新。
        window.refresh();
    })
}

pub(super) fn apply_host_effects(
    effects: Vec<HostEffect>,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    panel_runtimes: &PanelRuntimes,
    file_tree: &FileTreeRuntime,
    project_picker_runtime: &ProjectPickerRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let focus = FocusRouter::new(panel_runtimes, file_tree, editor_focus_fallback);
    for effect in effects {
        match effect {
            HostEffect::Quit => platform_window::quit(cx),
            HostEffect::Minimize => platform_window::minimize(window),
            HostEffect::ToggleMaximize => platform_window::toggle_maximize(window),
            HostEffect::TogglePanel(panel_str_id) => {
                let Some(panel) = PanelId::from_command_str_id(&panel_str_id) else {
                    eprintln!("HostEffect::TogglePanel 收到未知 panel id：{panel_str_id}");
                    continue;
                };
                let visible = workbench.borrow().is_panel_active(panel);
                if visible && focus.is_at(FocusTarget::Panel(panel), window) {
                    // 已显示且焦点就在它身上 —— 收起，焦点回编辑区。
                    workbench.borrow_mut().hide_panel(panel);
                    focus.move_to(FocusTarget::Editor, window);
                } else {
                    // 未显示，或虽显示但焦点不在它身上 —— 显示并把焦点交给它。
                    workbench.borrow_mut().show_panel(panel);
                    focus.move_to(FocusTarget::Panel(panel), window);
                }
                window.refresh();
            }
            HostEffect::ShowProjectPicker => {
                show_project_picker(
                    project_picker::ProjectPickerInitialMode::Browse,
                    app,
                    workbench,
                    surfaces,
                    editor_focus_fallback,
                    panel_runtimes,
                    file_tree,
                    project_picker_runtime,
                    window,
                    cx,
                );
            }
            HostEffect::OpenLocalProject => {
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
                    project_picker::ProjectPickerInitialMode::CloneGit,
                    app,
                    workbench,
                    surfaces,
                    editor_focus_fallback,
                    panel_runtimes,
                    file_tree,
                    project_picker_runtime,
                    window,
                    cx,
                );
            }
            HostEffect::RemoveSelectedRecentProject => {
                let picker_active = surfaces
                    .read_with(cx, |manager, _| manager.is_active(SurfaceId::ProjectPicker));
                if !picker_active {
                    continue;
                }
                let project_id = {
                    let projects = app.borrow().recent_projects();
                    project_picker_runtime.selected_project_id(&projects)
                };
                if let Some(project_id) = project_id {
                    app.borrow_mut().remove_recent_project(&project_id);
                    window.refresh();
                }
            }
            HostEffect::ShowLanguageServers => {
                open_surface(
                    language_servers::request(),
                    surfaces,
                    editor_focus_fallback,
                    window,
                    cx,
                );
            }
            HostEffect::ShowSettings => {
                eprintln!("设置界面尚未实现");
            }
            HostEffect::ShowDiagnostics => {
                eprintln!("诊断面板尚未实现");
            }
            HostEffect::DismissSurface => dismiss_surface(surfaces, window, cx),

            HostEffect::FileTreeMoveSelection(delta) => {
                app.borrow_mut().file_tree_move_selection(delta);
            }
            HostEffect::FileTreeCollapseOrParent => {
                app.borrow_mut().file_tree_collapse_or_parent();
            }
            HostEffect::FileTreeExpandOrInto => {
                app.borrow_mut().file_tree_expand_or_into();
            }
            HostEffect::FileTreeActivate => {
                let activation = app.borrow_mut().file_tree_activate();
                if matches!(activation, FileTreeActivation::OpenedFile) {
                    focus.move_to(FocusTarget::Editor, window);
                }
            }
            HostEffect::FileTreeFocusEditor => focus.move_to(FocusTarget::Editor, window),
            HostEffect::FileTreeBeginNewEntry(kind) => {
                app.borrow_mut().file_tree_begin_new_entry(kind);
            }
            HostEffect::FileTreeCommitNewEntry => {
                // 新建文件会被打开，焦点随之切到编辑器；新建目录留在文件树。
                let activation = app.borrow_mut().file_tree_commit_new_entry();
                if matches!(activation, FileTreeActivation::OpenedFile) {
                    focus.move_to(FocusTarget::Editor, window);
                }
            }
            HostEffect::FileTreeCancelNewEntry => {
                app.borrow_mut().file_tree_cancel_new_entry();
            }
            HostEffect::FileTreeRequestDelete => {
                app.borrow_mut().file_tree_request_delete();
            }
            HostEffect::FileTreeConfirmDelete => {
                app.borrow_mut().file_tree_confirm_delete();
            }
            HostEffect::FileTreeCancelDelete => {
                app.borrow_mut().file_tree_cancel_delete();
            }
        }
    }
}

pub(super) fn dismiss_surface(
    surfaces: &Entity<SurfaceManager>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let Some(focus_to_restore) = surfaces.update(cx, |surfaces, cx| surfaces.dismiss(cx)) else {
        return;
    };
    window.focus(&focus_to_restore);
    window.refresh();
}

fn show_project_picker(
    initial_mode: project_picker::ProjectPickerInitialMode,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    panel_runtimes: &PanelRuntimes,
    file_tree: &FileTreeRuntime,
    project_picker_runtime: &ProjectPickerRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let project_list_app = Rc::clone(app);
    let projects = Rc::new(move || project_list_app.borrow().recent_projects());
    let open_project_app = Rc::clone(app);
    let open_project_workbench = Rc::clone(workbench);
    let open_project_surfaces = surfaces.clone();
    let open_project_file_tree = file_tree.clone();
    let open_project = Rc::new(
        move |project_record: crate::app::RecentProject,
              window: &mut Window,
              cx: &mut gpui::App| {
            project::open_recent_project(
                Rc::clone(&open_project_app),
                Rc::clone(&open_project_workbench),
                &open_project_surfaces,
                open_project_file_tree.clone(),
                project_record.path,
                project_record.repo,
                window,
                cx,
            );
        },
    );
    let clone_app = Rc::clone(app);
    let clone_workbench = Rc::clone(workbench);
    let clone_surfaces = surfaces.clone();
    let clone_file_tree = file_tree.clone();
    let clone_git_project = Rc::new(
        move |repo: String, window: &mut Window, cx: &mut gpui::App| {
            project::clone_git_project(
                Rc::clone(&clone_app),
                Rc::clone(&clone_workbench),
                &clone_surfaces,
                clone_file_tree.clone(),
                repo,
                window,
                cx,
            );
        },
    );
    let key_app = Rc::clone(app);
    let key_workbench = Rc::clone(workbench);
    let key_surfaces = surfaces.clone();
    let key_editor_focus = editor_focus_fallback.clone();
    let key_panel_runtimes = panel_runtimes.clone();
    let key_file_tree = file_tree.clone();
    let key_project_picker = project_picker_runtime.clone();
    let key_request = Rc::new(
        move |chord: String, window: &mut Window, cx: &mut gpui::App| {
            let outcome = match key_app.borrow_mut().dispatch_key(chord, KeySurface::Panel) {
                Ok(outcome) => outcome,
                Err(error) => {
                    eprintln!("命令执行失败：{error}");
                    return false;
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
    let actions = project_picker::ProjectPickerActions {
        projects,
        open_project,
        clone_git_project,
        key_request,
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

fn open_surface(
    request: SurfaceRequest,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    // 手册 21.7：关闭时焦点回到"先前 focus 目标"——open 这一帧 window
    // 里实际聚焦的元素。查不到（窗口刚启动等）退回 editor 焦点，避免
    // 关闭后焦点悬空。
    let focus_to_restore = window
        .focused(cx)
        .unwrap_or_else(|| editor_focus_fallback.clone());
    let focus_on_open = request.focus_on_open.clone();
    surfaces.update(cx, |surfaces, cx| {
        surfaces.open(request, focus_to_restore, cx);
    });
    if let Some(focus) = focus_on_open {
        window.focus(&focus);
    }
    window.refresh();
}
