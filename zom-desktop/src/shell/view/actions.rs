//! ShellView 的命令动作与 HostEffect 解释。
//!
//! 此文件只承担 view 层壳：命令派发入口、HostEffect 总调度、跨 feature 的
//! 窗口 / surface 管理。每个 feature 自己的 HostEffect 处理都在
//! `features/<feature>/effects.rs` 里，由 [`apply_host_effects`] 按顺序问询。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, FocusHandle, Window};
use zom_command::{HostEffect, Invocation};

use crate::app::App;
use crate::focus::AppFocus;
use crate::shell::ActionRequest;
use crate::shell::features::language_servers::{self, LanguageServersRuntime};
use crate::shell::features::panels::file_tree::{self, FileTreeRuntime};
use crate::shell::features::panels::search;
use crate::shell::features::panels::{PanelId, PanelRuntimes};
use crate::shell::features::project_picker::{self, ProjectPickerRuntime};
use crate::shell::features::settings::{self, SettingsRuntime};
use crate::shell::platform::clipboard::GpuiClipboardScope;
use crate::shell::platform::window as platform_window;
use crate::shell::surfaces::{SurfaceId, SurfaceManager, SurfaceRequest};
use crate::shell::workbench::controller::WorkbenchController;

use super::focus::{FocusProjection, panel_default_focus, projection_from_runtimes};

pub(super) fn bind_action_request(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    surfaces: Entity<SurfaceManager>,
    editor_focus_fallback: FocusHandle,
    panel_runtimes: PanelRuntimes,
    file_tree: FileTreeRuntime,
    project_picker_runtime: ProjectPickerRuntime,
    language_servers_runtime: LanguageServersRuntime,
    settings_runtime: SettingsRuntime,
    invocation: Invocation,
) -> ActionRequest {
    Rc::new(move |window, cx| {
        let effects = {
            // 同 key_request：进入命令派发前借出 cx 给 GpuiClipboard。
            let _clip = GpuiClipboardScope::enter(cx);
            match app.borrow_mut().dispatch(invocation.clone()) {
                Ok(effects) => effects,
                Err(error) => {
                    eprintln!("命令执行失败：{error}");
                    return;
                }
            }
        };
        apply_host_effects_with_settings(
            effects,
            &app,
            &workbench,
            &surfaces,
            &editor_focus_fallback,
            &panel_runtimes,
            &file_tree,
            &project_picker_runtime,
            &language_servers_runtime,
            &settings_runtime,
            window,
            cx,
        );
        // 命令可能改了渲染可见的模型状态（如关闭删除确认弹窗）。
        // 与 key_request 的按键路径对称，点击路径在此统一刷新。
        window.refresh();
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_host_effects(
    effects: Vec<HostEffect>,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    panel_runtimes: &PanelRuntimes,
    file_tree: &FileTreeRuntime,
    project_picker_runtime: &ProjectPickerRuntime,
    language_servers_runtime: &LanguageServersRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let focus = projection_from_runtimes(
        editor_focus_fallback.clone(),
        panel_runtimes,
        file_tree,
        project_picker_runtime.focus_handle(),
        None,
    );
    for effect in effects {
        // 按 feature 顺序问询：第一个认领的 try_apply 返回 true，跳过余下。
        // 剩下的窗口控制 / 跨 feature 变体由本文件下方的兜底 match 处理。
        if file_tree::try_apply_effect(&effect, app, workbench, &focus, window) {
            continue;
        }
        if search::try_apply_effect(&effect, app, workbench, panel_runtimes, &focus, window) {
            continue;
        }
        if project_picker::try_apply_effect(
            &effect,
            app,
            workbench,
            surfaces,
            editor_focus_fallback,
            file_tree,
            project_picker_runtime,
            window,
            cx,
        ) {
            continue;
        }
        if language_servers::try_apply_effect(
            &effect,
            app,
            surfaces,
            editor_focus_fallback,
            language_servers_runtime,
            window,
            cx,
        ) {
            continue;
        }
        apply_shell_effect(&effect, app, workbench, surfaces, &focus, window, cx);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_host_effects_with_settings(
    effects: Vec<HostEffect>,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    panel_runtimes: &PanelRuntimes,
    file_tree: &FileTreeRuntime,
    project_picker_runtime: &ProjectPickerRuntime,
    language_servers_runtime: &LanguageServersRuntime,
    settings_runtime: &SettingsRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    for effect in effects {
        if settings::try_apply_effect(
            &effect,
            app,
            surfaces,
            editor_focus_fallback,
            settings_runtime,
            window,
            cx,
        ) {
            continue;
        }
        apply_host_effects(
            vec![effect],
            app,
            workbench,
            surfaces,
            editor_focus_fallback,
            panel_runtimes,
            file_tree,
            project_picker_runtime,
            language_servers_runtime,
            window,
            cx,
        );
    }
}

/// 没有归属到具体 feature 的"壳"级变体：窗口控制、TogglePanel、
/// DismissSurface、未实现的占位。
fn apply_shell_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    focus: &FocusProjection,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    match effect {
        HostEffect::Quit => platform_window::quit(cx),
        HostEffect::Minimize => platform_window::minimize(window),
        HostEffect::ToggleMaximize => platform_window::toggle_maximize(window),
        HostEffect::TogglePanel(panel_str_id) => {
            let Some(panel) = PanelId::from_command_str_id(panel_str_id) else {
                eprintln!("HostEffect::TogglePanel 收到未知 panel id：{panel_str_id}");
                return;
            };
            let visible = workbench.borrow().is_panel_active(panel);
            if visible && focus.is_at_panel(panel, window) {
                // 已显示且焦点就在它身上 —— 收起，焦点回编辑区。
                workbench.borrow_mut().hide_panel(panel);
                request_focus(app, focus, AppFocus::editor(), window);
            } else {
                // 未显示，或虽显示但焦点不在它身上 —— 显示并把焦点交给它。
                workbench.borrow_mut().show_panel(panel);
                request_focus(app, focus, panel_default_focus(panel), window);
            }
            window.refresh();
        }
        HostEffect::EditorToggleSoftWrap => {
            app.borrow_mut().toggle_soft_wrap();
            window.refresh();
        }
        HostEffect::DismissSurface => {
            if surfaces.read_with(cx, |manager, _| manager.is_active(SurfaceId::ProjectPicker)) {
                app.borrow_mut().project_picker_deactivate();
            }
            dismiss_surface(surfaces, window, cx);
        }
        HostEffect::ShowSettings => {
            eprintln!("设置界面尚未实现");
        }
        HostEffect::ShowDiagnostics => {
            eprintln!("诊断面板尚未实现");
        }
        other => {
            eprintln!("未处理的 HostEffect：{other:?}");
        }
    }
}

pub(crate) fn request_focus(
    app: &Rc<RefCell<App>>,
    projection: &FocusProjection,
    focus: AppFocus,
    window: &mut Window,
) {
    app.borrow_mut().request_focus(focus);
    let current = app.borrow().focus().current();
    projection.apply(current, window);
}

pub(crate) fn dismiss_surface(
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

pub(crate) fn open_surface(
    request: SurfaceRequest,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    // 手册 21.7：关闭时焦点回到"先前 focus 目标"——open 这一帧 window 里实际聚焦的元素。
    // 查不到（窗口刚启动等）退回 editor 焦点，避免关闭后焦点悬空。
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
