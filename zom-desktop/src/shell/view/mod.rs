//! shell 根视图。

pub(crate) mod actions;
mod config_visuals;
mod features;
pub(crate) mod focus;
mod frame_tick;
mod runtime;

use std::rc::Rc;

use gpui::{Context, FocusHandle, IntoElement, Render, ScrollHandle, Window};
use zom_command::Invocation;
use zom_command::commands::{file_tree as file_tree_commands, window as window_commands};

use crate::app::App;
use crate::clipboard::GpuiClipboardScope;
use crate::editor_state::build_editor_state;
use crate::focus::AppFocus;

use self::runtime::ShellRuntime;
use super::features::panels::file_tree::ConfirmDeleteHandlers;
use super::features::settings;
use super::workbench;
use super::workbench::WindowControlsHandlers;
use super::workbench::state::WorkbenchState;
use super::{ActionRequest, CommandCatalogLookup, CommandTitleLookup, KeyRequest, ShortcutLookup};
use crate::editor::{CaretBlink, drive_caret_blink};
use crate::ui_id::SurfaceId;

/// shell 端的根 View。装配产物收敛在 [`ShellRuntime`]；本结构再额外持
/// 几个跨帧但不入运行期组合根的视图态（编辑区标签栏滚动、光标闪烁）。
pub(crate) struct ShellView {
    runtime: ShellRuntime,
    /// 编辑区标签栏的滚动状态。跨帧保留，否则每帧重建会丢失滚动位置。
    editor_tab_scroll: ScrollHandle,
    /// 主编辑区光标闪烁状态，由本视图的定时链驱动。
    caret: CaretBlink,
}

impl ShellView {
    pub(super) fn new(app: App, cx: &mut Context<Self>) -> Self {
        Self {
            runtime: ShellRuntime::assemble(app, cx),
            editor_tab_scroll: ScrollHandle::new(),
            caret: CaretBlink::new(),
        }
    }

    pub(super) fn editor_focus(&self) -> FocusHandle {
        self.runtime.editor_focus.clone()
    }

    /// 注册 shell feature 需要挂到窗口上的监听器。
    pub(super) fn install_feature_listeners(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.runtime.features.install_listeners(
            Rc::clone(&self.runtime.app),
            self.runtime.surface_manager.clone(),
            window,
            cx,
        );
    }

    /// 打开指定路径的本地项目（不弹选择器）。开发阶段默认项目经由统一项目流程。
    /// 把启动期累积的配置 / 会话 / 最近项目气泡 flush 到 BubbleRuntime。
    /// 启动到第一帧之间无气泡 UI，调用方装好后再调用一次本方法。
    pub(super) fn flush_startup_bubbles(&self, window: &mut Window, cx: &mut gpui::App) {
        self.runtime.app.borrow_mut().pump_config_load_warnings();
        let mut requests = self.runtime.app.borrow_mut().take_session_bubbles();
        for warning in self.runtime.features.project_picker.take_recent_warnings() {
            requests.push(zom_command::BubbleRequest::error(warning).dedupe("project.recent"));
        }
        if requests.is_empty() {
            return;
        }
        for request in requests {
            self.runtime
                .bubble_runtime
                .update(cx, |runtime, cx| runtime.push(request, cx));
        }
        window.refresh();
    }

    #[cfg(debug_assertions)]
    pub(super) fn open_project(
        &self,
        project_root: std::path::PathBuf,
        window: &mut Window,
        cx: &mut gpui::App,
    ) {
        super::project_session::apply_local_project_open(
            &self.runtime.app,
            &self.runtime.workbench,
            &self.runtime.features.file_tree,
            &self.runtime.features.project_picker,
            &self.runtime.bubble_runtime,
            project_root,
            window,
            cx,
        );
    }

    fn workbench_state(&self) -> WorkbenchState {
        let app = self.runtime.app.borrow();
        self.runtime
            .workbench
            .borrow()
            .state(app.project_title(), app.has_project())
    }

    /// 把一个 [`Invocation`] 绑成 [`ActionRequest`]：点击时派发并应用窗口动作。
    fn bind_action(&self, invocation: Invocation) -> ActionRequest {
        actions::bind_action_request(
            Rc::clone(&self.runtime.app),
            Rc::clone(&self.runtime.workbench),
            self.runtime.surface_manager.clone(),
            self.runtime.bubble_runtime.clone(),
            self.runtime.editor_focus.clone(),
            self.runtime.features.clone(),
            invocation,
        )
    }

    fn window_controls_handlers(&self) -> WindowControlsHandlers {
        WindowControlsHandlers {
            quit: self.bind_action(window_commands::quit()),
            minimize: self.bind_action(window_commands::minimize()),
            toggle_maximize: self.bind_action(window_commands::toggle_maximize()),
        }
    }

    fn key_request(&self) -> KeyRequest {
        let app = Rc::clone(&self.runtime.app);
        let workbench = Rc::clone(&self.runtime.workbench);
        let surfaces = self.runtime.surface_manager.clone();
        let bubbles = self.runtime.bubble_runtime.clone();
        let editor_focus_fallback = self.runtime.editor_focus.clone();
        let features = self.runtime.features.clone();
        let focus_projection = self.runtime.focus_projection.clone();
        Rc::new(move |chord, window, cx| {
            let outcome = {
                // scope 内 GpuiClipboard 才能拿到 cx 访问系统剪贴板。
                // scope drop 后 thread-local 立即恢复，避免悬空。
                let _clip = GpuiClipboardScope::enter(cx);
                let current = focus_projection.current_focus(window);
                let mut app = app.borrow_mut();
                app.request_focus_from_shell(current);
                match app.dispatch_key(chord) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        eprintln!("命令执行失败：{error}");
                        return false;
                    }
                }
            };

            actions::apply_host_effects_with_settings(
                outcome.effects,
                &app,
                &workbench,
                &surfaces,
                &bubbles,
                &editor_focus_fallback,
                &features,
                window,
                cx,
            );
            if outcome.consumed {
                window.refresh();
            }
            outcome.consumed
        })
    }

    fn shortcut_lookup(&self) -> ShortcutLookup {
        let app = Rc::clone(&self.runtime.app);
        Rc::new(move |command_id| app.borrow().shortcuts_for(command_id))
    }

    fn command_title_lookup(&self) -> CommandTitleLookup {
        let app = Rc::clone(&self.runtime.app);
        Rc::new(move |command_id| app.borrow().command_title_for(command_id))
    }

    fn command_catalog_lookup(&self) -> CommandCatalogLookup {
        let app = Rc::clone(&self.runtime.app);
        Rc::new(move || app.borrow().command_catalog_items())
    }

    fn settings_action_request(&self) -> settings::SettingsActionRequest {
        let app = Rc::clone(&self.runtime.app);
        let editor_focus = self.runtime.editor_focus.clone();
        let bubbles = self.runtime.bubble_runtime.clone();
        Rc::new(move |action, window, cx| {
            match action {
                settings::SettingsAction::OpenToml => {
                    let opened = app.borrow_mut().open_config_file();
                    for request in app.borrow_mut().take_session_bubbles() {
                        bubbles.update(cx, |runtime, cx| runtime.push(request, cx));
                    }
                    if opened {
                        window.focus(&editor_focus);
                    }
                }
                settings::SettingsAction::Change(change) => {
                    let config = {
                        let mut app = app.borrow_mut();
                        app.apply_settings_change(change);
                        app.config_snapshot()
                    };
                    config_visuals::apply(&config);
                }
            }
            window.refresh();
        })
    }
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let runtime = &self.runtime;
        // GPUI → App 单向反向同步：点击 / Tab / 系统焦点变化只动 FocusHandle，不经过 App。
        // 每帧渲染开头把 projection 当前焦点拉回 FocusStore，
        // 本帧的状态栏、命令面板可见性、IME 路由读到的就是真值，不会落后一帧。
        // key_request 在派发命令前另有一次同步——两次都幂等，保留作为兜底。
        let projected = runtime.focus_projection.current_focus(window);
        {
            let mut app = runtime.app.borrow_mut();
            app.request_focus_from_shell(projected);
            frame_tick::advance(&mut app);
        }

        let state = self.workbench_state();
        // 三个 feature 的视图快照旁路收集，不进 WorkbenchState；workbench::render 只看布局。
        let editor_state = runtime
            .app
            .borrow()
            .with_workspace_views(build_editor_state);
        let file_tree_state = {
            let app = runtime.app.borrow();
            runtime.features.file_tree.state(&app)
        };
        let search_state = {
            let app = runtime.app.borrow();
            runtime
                .features
                .search
                .runtime_handle()
                .state(app.workspace())
        };

        // 光标一移动就重置闪烁为实心，让用户立刻定位到光标；定时链与全局可见位都由 editor 子系统驱动。
        // 现在由全局唯一的 AppFocus 作为真相源，精确向路由查询当前焦点对应的快照。
        let active_cursor = {
            let app = runtime.app.borrow();
            let current_focus = app.focus().current();
            app.with_router(|router| router.snapshot_for_focus(current_focus).cursor_byte)
        };

        drive_caret_blink(&mut self.caret, active_cursor, cx, |view| &mut view.caret);
        let window_controls = self.window_controls_handlers();
        let key_request = self.key_request();
        let runtime = &self.runtime;
        runtime
            .features
            .settings
            .set_key_request(Rc::clone(&key_request));
        runtime
            .features
            .settings
            .set_action_request(self.settings_action_request());
        runtime.features.settings.set_state({
            let app = runtime.app.borrow();
            settings::SettingsPanelState::new(app.config_snapshot(), app.config_path())
        });
        runtime
            .features
            .project_picker
            .set_key_request(Rc::clone(&key_request));
        runtime
            .features
            .language_servers
            .set_key_request(Rc::clone(&key_request));
        // file_tree_panel 借用此 clone；下面把 `key_request` 本体 move 给 `workbench::render`。
        // 借用与移动落到不同的 Rc 副本上，互不冲突。
        let key_request_for_panel = Rc::clone(&key_request);
        let file_tree_panel = runtime.features.file_tree.panel(
            &file_tree_state,
            &key_request_for_panel,
            &runtime.file_tree_new_entry_slot,
            &runtime.file_tree_rename_slot,
            window,
        );
        let shortcut_lookup = self.shortcut_lookup();
        let command_title_lookup = self.command_title_lookup();
        let command_catalog_lookup = self.command_catalog_lookup();
        let workspace_active = runtime
            .surface_manager
            .read_with(cx, |manager, _| manager.is_active(SurfaceId::ProjectPicker));
        let language_server_active = runtime.surface_manager.read_with(cx, |manager, _| {
            manager.is_active(SurfaceId::LanguageServers)
        });
        let settings_active = runtime
            .surface_manager
            .read_with(cx, |manager, _| manager.is_active(SurfaceId::Settings));
        let confirm_delete = ConfirmDeleteHandlers {
            confirm: self.bind_action(file_tree_commands::confirm_delete()),
            cancel: self.bind_action(file_tree_commands::cancel_delete()),
        };
        let main_editor_snapshot = {
            let app = runtime.app.borrow();
            app.with_router(|router| router.snapshot_for_focus(AppFocus::editor()))
        };
        workbench::render(
            &state,
            workbench::WorkbenchFeatureStates {
                editor: &editor_state,
                file_tree: &file_tree_state,
                search: &search_state,
            },
            &runtime.panel_host,
            Rc::clone(&runtime.workbench),
            window,
            window_controls,
            runtime.surface_shell.clone(),
            runtime.bubble_shell.clone(),
            workspace_active,
            settings_active,
            language_server_active,
            key_request,
            shortcut_lookup,
            command_title_lookup,
            command_catalog_lookup,
            Rc::clone(&runtime.main_editor_slot),
            Rc::clone(&runtime.search_query_slot),
            Rc::clone(&runtime.search_replacement_slot),
            runtime.editor_focus.clone(),
            runtime.features.panels.clone(),
            runtime.features.search.clone(),
            file_tree_panel,
            self.editor_tab_scroll.clone(),
            confirm_delete,
            main_editor_snapshot,
        )
    }
}
