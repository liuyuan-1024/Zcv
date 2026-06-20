//! shell 根视图。

pub(crate) mod actions;
mod config_visuals;
mod features;
pub(crate) mod focus;
mod frame_tick;
mod runtime;

use std::rc::Rc;

use gpui::{Context, FocusHandle, IntoElement, Render, ScrollHandle, Subscription, Window};
use zom_command::commands::{
    diagnostics as diagnostics_commands, editor as editor_commands,
    file_tree as file_tree_commands, go_to_line as go_to_line_commands,
    language_servers as language_server_commands, project_picker as project_picker_commands,
    search::{file as search_file_commands, project as search_project_commands},
    settings as settings_commands, window as window_commands,
};
use zom_command::{CommandArgs, CommandId, Invocation, SettingsChangeRequest};
use zom_workspace::view::ViewId;

use crate::{app::App, theme::Theme};
use crate::{config, focus::AppFocus};

use self::runtime::ShellRuntime;
use super::features::panels::file_tree::ConfirmDeleteHandlers;
use super::features::search::{SearchIntent, SearchIntentRequest};
use super::features::settings;
use super::shared::CommandBinding;
use super::surfaces::SurfaceStates;
use super::workbench;
use super::workbench::WindowControlsHandlers;
use super::workbench::WorkbenchCommandRequests;
use super::workbench::state::WorkbenchState;
use super::{CommandCatalogLookup, CommandTitleLookup, ShortcutLookup};
use crate::editor::{CaretBlink, drive_caret_blink};
use crate::host_intent::{
    CommandRequest, FileTreeClickCallback, HostIntent, HostIntentRequest, KeyRequest, TabCallback,
};
use crate::ui_id::{PanelId, SurfaceId};

/// shell 端的根 View。装配产物收敛在 [`ShellRuntime`]；
/// 本结构再额外持几个跨帧但不入运行期组合根的视图态（编辑区标签栏滚动、光标闪烁）。
pub(crate) struct ShellView {
    runtime: ShellRuntime,
    /// 编辑区标签栏的滚动状态。跨帧保留，否则每帧重建会丢失滚动位置。
    editor_tab_scroll: ScrollHandle,
    /// 主编辑区光标闪烁状态，由本视图的定时链驱动。
    caret: CaretBlink,
    /// 系统外观变化订阅。持有即保持回调存活，Drop 时自动取消。
    _appearance_subscription: Option<Subscription>,
}

impl ShellView {
    pub(super) fn new(app: App, cx: &mut Context<Self>) -> Self {
        Self {
            runtime: ShellRuntime::assemble(app, cx),
            editor_tab_scroll: ScrollHandle::new(),
            caret: CaretBlink::new(),
            _appearance_subscription: None,
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
        // 窗口就绪后立即用正确的系统外观解析 "system" 主题。
        {
            let config = self.runtime.app.borrow().config_snapshot();
            config_visuals::apply(&config, Some(window));
        }

        self.runtime
            .features
            .install_listeners(Rc::clone(&self.runtime.app), window, cx);

        // 系统外观变化时自动跟随。
        let app = Rc::clone(&self.runtime.app);
        self._appearance_subscription =
            Some(cx.observe_window_appearance(window, move |_, window, cx| {
                let config = app.borrow().config_snapshot();
                if Theme::from_config(&config.general.theme).is_system() {
                    config_visuals::apply(&config, Some(window));
                    cx.notify();
                }
            }));
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

    /// 把一个 [`Invocation`] 绑成 [`CommandRequest`]：触发时进入命令管线。
    fn bind_command(&self, invocation: Invocation) -> CommandRequest {
        actions::bind_command_request(self.host_intent_request(), invocation)
    }

    fn host_intent_request(&self) -> HostIntentRequest {
        Rc::clone(&self.runtime.host_intent)
    }

    fn search_intent_request(&self) -> SearchIntentRequest {
        let toggle_case_sensitive =
            self.bind_command(search_file_commands::toggle_case_sensitive());
        let toggle_whole_word = self.bind_command(search_file_commands::toggle_whole_word());
        let toggle_regex = self.bind_command(search_file_commands::toggle_regex());
        let find_previous = self.bind_command(search_file_commands::find_previous());
        let find_next = self.bind_command(search_file_commands::find_next());
        let replace_next = self.bind_command(search_file_commands::replace_next());
        let replace_all = self.bind_command(search_file_commands::replace_all());

        Rc::new(move |intent, window, cx| {
            let request = match intent {
                SearchIntent::ToggleCaseSensitive => &toggle_case_sensitive,
                SearchIntent::ToggleWholeWord => &toggle_whole_word,
                SearchIntent::ToggleRegex => &toggle_regex,
                SearchIntent::FindPrevious => &find_previous,
                SearchIntent::FindNext => &find_next,
                SearchIntent::ReplaceNext => &replace_next,
                SearchIntent::ReplaceAll => &replace_all,
            };
            request(window, cx);
        })
    }

    fn workbench_command_requests(&self) -> WorkbenchCommandRequests {
        let title = self.command_title_lookup();
        let shortcut = self.shortcut_lookup();

        let binding = |cmd_id: &'static str, inv: Invocation| -> CommandBinding {
            CommandBinding {
                id: cmd_id.to_string(),
                title: Rc::clone(&title),
                shortcut: Rc::clone(&shortcut),
                request: self.bind_command(inv),
            }
        };

        let panel_toggle = Rc::new({
            let this = Rc::clone(&self.runtime.host_intent);
            move |panel: PanelId| {
                let command_id =
                    CommandId::new(panel.toggle_command_id()).expect("内建命令 ID 必须非空");
                actions::bind_command_request(
                    Rc::clone(&this),
                    (command_id, CommandArgs::new().with("via", "pointer")),
                )
            }
        });

        let tab_select: TabCallback = {
            let app = Rc::clone(&self.runtime.app);
            Rc::new(move |view_id, window, _cx| {
                app.borrow_mut().activate_view_tab(view_id);
                window.refresh();
            })
        };

        let tab_close: Rc<dyn Fn(ViewId) -> CommandBinding> = {
            let host_intent = self.host_intent_request();
            let title = Rc::clone(&title);
            let shortcut = Rc::clone(&shortcut);
            Rc::new(move |view_id| {
                let request = actions::bind_command_request(
                    Rc::clone(&host_intent),
                    editor_commands::close_tab_by_id(view_id),
                );
                CommandBinding {
                    id: editor_commands::CLOSE_TAB.to_string(),
                    title: Rc::clone(&title),
                    shortcut: Rc::clone(&shortcut),
                    request,
                }
            })
        };

        WorkbenchCommandRequests {
            project_picker_open: binding(
                project_picker_commands::SHOW_PROJECTS_PICKER,
                project_picker_commands::show_projects_picker(),
            ),
            settings_open: binding(settings_commands::OPEN, settings_commands::open()),
            language_servers_open: binding(
                language_server_commands::OPEN,
                language_server_commands::open(),
            ),
            diagnostics_show_problems: binding(
                diagnostics_commands::SHOW_PROBLEMS,
                diagnostics_commands::show_problems(),
            ),
            project_search_activate: binding(
                search_project_commands::PROJECT_ACTIVATE,
                search_project_commands::project_activate(),
            ),
            editor_open_preview: binding(
                editor_commands::OPEN_PREVIEW,
                editor_commands::open_preview(),
            ),
            file_search_activate: binding(
                search_file_commands::ACTIVATE,
                search_file_commands::activate(),
            ),
            file_search_dismiss: binding(
                search_file_commands::DISMISS,
                search_file_commands::dismiss(),
            ),
            editor_go_to_line: binding(
                go_to_line_commands::ACTIVATE,
                go_to_line_commands::activate(),
            ),
            editor_change_language: binding(
                editor_commands::CHANGE_LANGUAGE,
                editor_commands::change_language(),
            ),
            panel_toggle,
            search_intent: self.search_intent_request(),
            shortcut_lookup: shortcut,
            title_lookup: title,
            tab_select,
            tab_close,
        }
    }

    fn window_controls_handlers(&self) -> WindowControlsHandlers {
        WindowControlsHandlers {
            quit: self.bind_command(window_commands::quit()),
            minimize: self.bind_command(window_commands::minimize()),
            toggle_maximize: self.bind_command(window_commands::toggle_maximize()),
        }
    }

    fn key_request(&self) -> KeyRequest {
        actions::bind_key_request(self.host_intent_request())
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

    fn settings_intent_request(&self) -> settings::SettingsIntentRequest {
        let host_intent = self.host_intent_request();
        Rc::new(move |intent, window, cx| {
            let invocation = settings_intent_invocation(intent);
            host_intent(HostIntent::Command(invocation), window, cx);
        })
    }
}

fn settings_intent_invocation(intent: settings::SettingsIntent) -> Invocation {
    match intent {
        settings::SettingsIntent::OpenToml => settings_commands::open_toml(),
        settings::SettingsIntent::Change(change) => {
            settings_commands::apply_change(settings_change_request(change))
        }
    }
}

fn settings_change_request(change: config::SettingsChange) -> SettingsChangeRequest {
    match change {
        config::SettingsChange::AdjustUiFont(delta) => SettingsChangeRequest::AdjustUiFont(delta),
        config::SettingsChange::AdjustEditorFont(delta) => {
            SettingsChangeRequest::AdjustEditorFont(delta)
        }
        config::SettingsChange::ToggleEditorSoftWrap => SettingsChangeRequest::ToggleEditorSoftWrap,
        config::SettingsChange::CycleEditorTabSize => SettingsChangeRequest::CycleEditorTabSize,
        config::SettingsChange::CycleTheme => SettingsChangeRequest::CycleTheme,
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
        let editor_state = runtime.app.borrow().editor_state();
        let file_tree_state = {
            let app = runtime.app.borrow();
            runtime.features.file_tree.state(&app)
        };
        let search_state = {
            let app = runtime.app.borrow();
            runtime.features.search.runtime_handle().state(
                app.workspace(),
                app.views(),
                app.active_view_id(),
            )
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
            .set_intent_request(self.settings_intent_request());
        runtime
            .features
            .settings
            .set_title_lookup(self.command_title_lookup());
        runtime
            .features
            .settings
            .set_shortcut_lookup(self.shortcut_lookup());
        runtime.features.settings.set_state({
            let app = runtime.app.borrow();
            settings::SettingsPanelState::new(app.config_snapshot(), app.config_path())
        });
        runtime.surface_shell.update(cx, |shell, _| {
            shell.set_key_request(Rc::clone(&key_request))
        });
        // file_tree_panel 借用此 clone；下面把 `key_request` 本体 move 给 `workbench::render`。
        // 借用与移动落到不同的 Rc 副本上，互不冲突。
        let key_request_for_panel = Rc::clone(&key_request);
        let on_item_click: FileTreeClickCallback = {
            let file_tree = runtime.features.file_tree.clone();
            let activate = self.bind_command(file_tree_commands::activate());
            Rc::new(move |path, window, cx| {
                file_tree.select(path);
                activate(window, cx);
            })
        };
        let file_tree_panel = runtime.features.file_tree.panel(
            &file_tree_state,
            &key_request_for_panel,
            &runtime.file_tree_new_entry_slot,
            &runtime.file_tree_rename_slot,
            &on_item_click,
            window,
        );
        let shortcut_lookup = self.shortcut_lookup();
        let command_title_lookup = self.command_title_lookup();
        let command_catalog_lookup = self.command_catalog_lookup();
        let surface_states = SurfaceStates {
            project_picker: runtime
                .surface_manager
                .read_with(cx, |manager, _| manager.is_active(SurfaceId::ProjectPicker)),
            settings: runtime
                .surface_manager
                .read_with(cx, |manager, _| manager.is_active(SurfaceId::Settings)),
            language_servers: runtime.surface_manager.read_with(cx, |manager, _| {
                manager.is_active(SurfaceId::LanguageServers)
            }),
            go_to_line: runtime
                .surface_manager
                .read_with(cx, |manager, _| manager.is_active(SurfaceId::GoToLine)),
        };
        let confirm_delete = ConfirmDeleteHandlers {
            confirm: self.bind_command(file_tree_commands::confirm_delete()),
            cancel: self.bind_command(file_tree_commands::cancel_delete()),
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
            self.workbench_command_requests(),
            &runtime.panel_host,
            Rc::clone(&runtime.workbench),
            window,
            window_controls,
            runtime.surface_shell.clone(),
            runtime.bubble_shell.clone(),
            &surface_states,
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
