//! shell 根视图。

pub(crate) mod actions;
pub(crate) mod focus;
pub(crate) mod project;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AppContext, BorrowAppContext, Context, Entity, FocusHandle, IntoElement, Render, ScrollHandle,
    Window,
};
use zom_command::Invocation;
use zom_command::commands::{file_tree as file_tree_commands, window as window_commands};

use crate::app::App;
use crate::focus::{AppFocus, FileTreeFocus, ProjectPickerFocus, SearchField};
use crate::shell::platform::clipboard::{GpuiClipboard, GpuiClipboardScope};

use self::focus::{FocusProjection, projection_from_runtimes};
use super::editor::{
    CaretBlink, EditorKernel, EditorViewportSyncHook, TextEditorSlot, drive_caret_blink,
};
use super::features::language_servers;
use super::features::panels::PanelRuntimes;
use super::features::panels::file_tree::{ConfirmDeleteHandlers, FileTreeRuntime};
use super::features::project_picker::ProjectPickerRuntime;
use super::surfaces::{SurfaceAnchorRegistry, SurfaceId, SurfaceManager, SurfaceShell};
use super::workbench;
use super::workbench::controller::WorkbenchController;
use super::workbench::state::WorkbenchState;
use super::workbench::{PanelHost, WindowControlsHandlers};
use super::{ActionRequest, CommandCatalogLookup, CommandTitleLookup, KeyRequest, ShortcutLookup};

/// shell 端的根 View：拥有 App 状态与每窗口的 `PanelHost`。
pub(crate) struct ShellView {
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    panel_host: PanelHost,
    surface_manager: Entity<SurfaceManager>,
    surface_shell: Entity<SurfaceShell>,
    main_editor_slot: Rc<TextEditorSlot>,
    file_tree_slot: Rc<TextEditorSlot>,
    project_picker_slot: Rc<TextEditorSlot>,
    search_query_slot: Rc<TextEditorSlot>,
    search_replacement_slot: Rc<TextEditorSlot>,
    editor_focus: FocusHandle,
    panel_runtimes: PanelRuntimes,
    file_tree: FileTreeRuntime,
    project_picker: ProjectPickerRuntime,
    language_servers: language_servers::LanguageServersRuntime,
    /// 编辑区标签栏的滚动状态。跨帧保留，否则每帧重建会丢失滚动位置。
    editor_tab_scroll: ScrollHandle,
    /// 主编辑区光标闪烁状态，由本视图的定时链驱动。
    caret: CaretBlink,
    /// AppFocus 与 GPUI FocusHandle 之间的窗口系统投影表。
    focus_projection: FocusProjection,
}

impl ShellView {
    pub(super) fn new(app: App, cx: &mut Context<Self>) -> Self {
        let app = Rc::new(RefCell::new(app));
        // 让命令派发期间的 copy / cut / paste 走系统剪贴板；headless 单测路径
        // 不经过 ShellView::new，所以仍是 MockClipboard。
        app.borrow_mut().set_clipboard(Box::new(GpuiClipboard));
        let workbench = Rc::new(RefCell::new(WorkbenchController::new()));
        cx.update_default_global::<SurfaceAnchorRegistry, _>(|_, _| ());
        let surface_manager = cx.new(|_| SurfaceManager::new());
        let editor_focus = cx.focus_handle();
        let panel_runtimes = PanelRuntimes::new(cx);
        let file_tree = FileTreeRuntime::new(cx);
        let project_picker = ProjectPickerRuntime::new(cx);
        let language_servers = language_servers::LanguageServersRuntime::new(cx);

        // 主编辑区内核：多行 + 行号 + 滚动 + 视口写回。视口钩子在 prepaint
        // 末尾把测得的 visible_line_count 推回 view 的 ViewportState；top_line
        // 由 view 自己在 settle 阶段落定，element 不写它。
        let main_viewport_sync: EditorViewportSyncHook = {
            let app = Rc::clone(&app);
            Rc::new(move |visible_line_count, _cx| {
                app.borrow_mut()
                    .set_main_visible_line_count(visible_line_count);
            })
        };
        let main_editor_slot = TextEditorSlot::install(
            Rc::clone(&app),
            AppFocus::editor(),
            EditorKernel::multi_line()
                .with_gutter()
                .with_vertical_scroll()
                .with_viewport_sync(main_viewport_sync),
            editor_focus.clone(),
            cx,
        );
        let file_tree_slot = TextEditorSlot::install(
            Rc::clone(&app),
            AppFocus::file_tree(FileTreeFocus::NewEntryName),
            EditorKernel::single_line(),
            file_tree.focus_handle(),
            cx,
        );
        let project_picker_slot = TextEditorSlot::install(
            Rc::clone(&app),
            AppFocus::project_picker(ProjectPickerFocus::Query),
            EditorKernel::single_line(),
            project_picker.focus_handle(),
            cx,
        );
        let search_query_slot = TextEditorSlot::install(
            Rc::clone(&app),
            AppFocus::search(SearchField::Query),
            EditorKernel::single_line(),
            panel_runtimes.search_query_focus_handle(),
            cx,
        );
        let search_replacement_slot = TextEditorSlot::install(
            Rc::clone(&app),
            AppFocus::search(SearchField::Replacement),
            EditorKernel::single_line(),
            panel_runtimes.search_replacement_focus_handle(),
            cx,
        );

        let surface_shell = cx.new(|cx| SurfaceShell::new(surface_manager.clone(), cx));

        let focus_projection = projection_from_runtimes(
            editor_focus.clone(),
            &panel_runtimes,
            &file_tree,
            project_picker.focus_handle(),
        );

        Self {
            app,
            workbench,
            panel_host: PanelHost::new(),
            surface_manager,
            surface_shell,
            main_editor_slot,
            file_tree_slot,
            project_picker_slot,
            search_query_slot,
            search_replacement_slot,
            editor_focus,
            panel_runtimes,
            file_tree,
            project_picker,
            language_servers,
            editor_tab_scroll: ScrollHandle::new(),
            caret: CaretBlink::new(),
            focus_projection,
        }
    }

    pub(super) fn editor_focus(&self) -> FocusHandle {
        self.editor_focus.clone()
    }

    /// 注册 shell feature 需要挂到窗口上的监听器。
    pub(super) fn install_feature_listeners(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.file_tree
            .install_listeners(Rc::clone(&self.app), window, cx);
        self.project_picker.install_listeners(
            Rc::clone(&self.app),
            self.surface_manager.clone(),
            window,
            cx,
        );
        self.language_servers
            .install_listeners(self.surface_manager.clone(), window, cx);
        self.panel_runtimes
            .install_listeners(Rc::clone(&self.app), window, cx);
    }

    /// 打开指定路径的本地项目（不弹选择器）。开发阶段默认项目经由统一项目流程。
    #[cfg(debug_assertions)]
    pub(super) fn open_project(&self, project_root: std::path::PathBuf, window: &mut Window) {
        project::apply_local_project_open(
            &self.app,
            &self.workbench,
            &self.file_tree,
            project_root,
            window,
        );
    }

    fn workbench_state(&self) -> WorkbenchState {
        let app = self.app.borrow();
        self.workbench.borrow().state(
            app.project_title(),
            app.has_project(),
            app.editor_state(),
            app.file_tree_state(),
            app.search_state(),
        )
    }

    /// 把一个 [`Invocation`] 绑成 [`ActionRequest`]：点击时派发并应用窗口动作。
    fn bind_action(&self, invocation: Invocation) -> ActionRequest {
        actions::bind_action_request(
            Rc::clone(&self.app),
            Rc::clone(&self.workbench),
            self.surface_manager.clone(),
            self.editor_focus.clone(),
            self.panel_runtimes.clone(),
            self.file_tree.clone(),
            self.project_picker.clone(),
            self.language_servers.clone(),
            Rc::clone(&self.project_picker_slot),
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
        let app = Rc::clone(&self.app);
        let workbench = Rc::clone(&self.workbench);
        let surfaces = self.surface_manager.clone();
        let editor_focus_fallback = self.editor_focus.clone();
        let panel_runtimes = self.panel_runtimes.clone();
        let file_tree = self.file_tree.clone();
        let project_picker = self.project_picker.clone();
        let language_servers = self.language_servers.clone();
        let project_picker_slot = Rc::clone(&self.project_picker_slot);
        let focus_projection = self.focus_projection.clone();
        Rc::new(move |chord, window, cx| {
            let outcome = {
                // scope 内 GpuiClipboard 才能拿到 cx 访问系统剪贴板；scope drop
                // 后 thread-local 立即恢复，避免悬空。
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

            actions::apply_host_effects(
                outcome.effects,
                &app,
                &workbench,
                &surfaces,
                &editor_focus_fallback,
                &panel_runtimes,
                &file_tree,
                &project_picker,
                &language_servers,
                &project_picker_slot,
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
        let app = Rc::clone(&self.app);
        Rc::new(move |command_id| app.borrow().shortcut_for(command_id))
    }

    fn command_title_lookup(&self) -> CommandTitleLookup {
        let app = Rc::clone(&self.app);
        Rc::new(move |command_id| app.borrow().command_title_for(command_id))
    }

    fn command_catalog_lookup(&self) -> CommandCatalogLookup {
        let app = Rc::clone(&self.app);
        Rc::new(move || app.borrow().command_catalog_items())
    }
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // GPUI → App 单向反向同步：点击 / Tab / 系统焦点变化只动 FocusHandle，
        // 不经过 App。每帧渲染开头把 projection 当前焦点拉回 FocusStore，本帧的
        // 状态栏、命令面板可见性、IME 路由读到的就是真值，不会落后一帧。
        // key_request 在派发命令前另有一次同步——两次都幂等，保留作为兜底。
        let projected = self.focus_projection.current_focus(window);
        self.app.borrow_mut().request_focus_from_shell(projected);

        let state = self.workbench_state();

        // 光标一移动就重置闪烁为实心，让用户立刻定位到光标；定时链与全局可见位都由 editor 子系统驱动
        // 现在由全局唯一的 AppFocus 作为真相源，精确向路由查询当前焦点对应的快照。
        let active_cursor = {
            let app = self.app.borrow();
            let current_focus = app.focus().current();
            app.with_router(|router| router.snapshot_for_focus(current_focus).cursor_byte)
        };

        drive_caret_blink(&mut self.caret, active_cursor, cx, |view| &mut view.caret);
        let window_controls = self.window_controls_handlers();
        let key_request = self.key_request();
        // file_tree_panel 借用此 clone；下面把 `key_request` 本体 move 给
        // `workbench::render` —— 借用与移动落到不同的 Rc 副本上，互不冲突。
        let key_request_for_panel = Rc::clone(&key_request);
        let file_tree_panel = self.file_tree.panel(
            &state.file_tree,
            &key_request_for_panel,
            &self.file_tree_slot,
            window,
        );
        let shortcut_lookup = self.shortcut_lookup();
        let command_title_lookup = self.command_title_lookup();
        let command_catalog_lookup = self.command_catalog_lookup();
        let workspace_active = self
            .surface_manager
            .read_with(cx, |manager, _| manager.is_active(SurfaceId::ProjectPicker));
        let language_server_active = self.surface_manager.read_with(cx, |manager, _| {
            manager.is_active(SurfaceId::LanguageServers)
        });
        let confirm_delete = ConfirmDeleteHandlers {
            confirm: self.bind_action(file_tree_commands::confirm_delete()),
            cancel: self.bind_action(file_tree_commands::cancel_delete()),
        };
        let main_editor_snapshot = {
            let app = self.app.borrow();
            app.with_router(|router| router.snapshot_for_focus(AppFocus::editor()))
        };
        workbench::render(
            &state,
            &self.panel_host,
            Rc::clone(&self.workbench),
            window,
            window_controls,
            self.surface_shell.clone(),
            workspace_active,
            language_server_active,
            key_request,
            shortcut_lookup,
            command_title_lookup,
            command_catalog_lookup,
            Rc::clone(&self.main_editor_slot),
            Rc::clone(&self.search_query_slot),
            Rc::clone(&self.search_replacement_slot),
            self.editor_focus.clone(),
            self.panel_runtimes.clone(),
            file_tree_panel,
            self.editor_tab_scroll.clone(),
            confirm_delete,
            main_editor_snapshot,
        )
    }
}
