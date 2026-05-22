//! shell 根视图。

mod actions;
mod focus;
mod project;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AppContext, Context, ElementInputHandler, Entity, FocusHandle, IntoElement, Render,
    ScrollHandle, Window,
};
use zom_command::Invocation;
use zom_command::commands::{file_tree as file_tree_commands, window as window_commands};

use crate::app::{App, KeySurface};

use super::editor::{CARET_BLINK_INTERVAL, CaretBlink, EditorInput};
use super::features::PanelRuntimes;
use super::features::file_tree::{ConfirmDeleteHandlers, FileTreeRuntime};
use super::workbench;
use super::workbench::controller::WorkbenchController;
use super::workbench::overlays::{AnchorRegistry, OverlayKind, OverlayManager, OverlayShell};
use super::workbench::state::WorkbenchState;
use super::workbench::{PanelHost, WindowControlsHandlers};
use super::{ActionRequest, InputHandlerHook, KeyRequest, ShortcutLookup};

/// shell 端的根 View：拥有 App 状态与每窗口的 `PanelHost`。
pub(crate) struct ShellView {
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    panel_host: PanelHost,
    overlay_manager: Entity<OverlayManager>,
    anchor_registry: Entity<AnchorRegistry>,
    overlay_shell: Entity<OverlayShell>,
    editor_input: Entity<EditorInput>,
    editor_focus: FocusHandle,
    panel_runtimes: PanelRuntimes,
    file_tree: FileTreeRuntime,
    /// 编辑区标签栏的滚动状态。跨帧保留，否则每帧重建会丢失滚动位置。
    editor_tab_scroll: ScrollHandle,
    /// 主编辑区光标闪烁状态，由本视图的定时链驱动。
    caret: CaretBlink,
}

impl ShellView {
    pub(super) fn new(app: App, cx: &mut Context<Self>) -> Self {
        let app = Rc::new(RefCell::new(app));
        let workbench = Rc::new(RefCell::new(WorkbenchController::new()));
        let overlay_manager = cx.new(|_| OverlayManager::new());
        let anchor_registry = cx.new(|_| AnchorRegistry::new());
        let editor_focus = cx.focus_handle();
        let editor_input = cx.new(|_| EditorInput::new(Rc::clone(&app)));
        let panel_runtimes = PanelRuntimes::new(cx);
        let file_tree = FileTreeRuntime::new(cx);
        let open_local_project = actions::bind_action_request(
            Rc::clone(&app),
            Rc::clone(&workbench),
            overlay_manager.clone(),
            editor_focus.clone(),
            panel_runtimes.clone(),
            file_tree.clone(),
            zom_command::commands::workspace::open_local_project(),
        );
        let overlay_shell = cx.new(|cx| {
            OverlayShell::new(
                overlay_manager.clone(),
                anchor_registry.clone(),
                open_local_project,
                cx,
            )
        });

        Self {
            app,
            workbench,
            panel_host: PanelHost::new(),
            overlay_manager,
            anchor_registry,
            overlay_shell,
            editor_input,
            editor_focus,
            panel_runtimes,
            file_tree,
            editor_tab_scroll: ScrollHandle::new(),
            caret: CaretBlink::new(),
        }
    }

    /// 调度一次光标闪烁定时翻转。每次翻转后自行续链；`epoch` 不符（光标
    /// 移动触发过 [`CaretBlink::restart`]）时旧链自然终止。
    fn schedule_caret_blink(&self, epoch: usize, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CARET_BLINK_INTERVAL).await;
            this.update(cx, |this, cx| {
                if this.caret.tick(epoch) {
                    cx.notify();
                    this.schedule_caret_blink(epoch, cx);
                }
            })
            .ok();
        })
        .detach();
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
    }

    /// 打开指定路径的本地项目（不弹选择器）。开发阶段默认项目经由统一项目流程。
    #[cfg(debug_assertions)]
    pub(super) fn open_project(&self, project_root: std::path::PathBuf, window: &mut Window) {
        project::apply_project_open(
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
        )
    }

    /// 把一个 [`Invocation`] 绑成 [`ActionRequest`]：点击时派发并应用窗口动作。
    fn bind_action(&self, invocation: Invocation) -> ActionRequest {
        actions::bind_action_request(
            Rc::clone(&self.app),
            Rc::clone(&self.workbench),
            self.overlay_manager.clone(),
            self.editor_focus.clone(),
            self.panel_runtimes.clone(),
            self.file_tree.clone(),
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

    fn key_request(&self, surface: KeySurface) -> KeyRequest {
        let app = Rc::clone(&self.app);
        let workbench = Rc::clone(&self.workbench);
        let overlays = self.overlay_manager.clone();
        let editor_focus_fallback = self.editor_focus.clone();
        let panel_runtimes = self.panel_runtimes.clone();
        let file_tree = self.file_tree.clone();
        Rc::new(move |chord, window, cx| {
            let outcome = match app.borrow_mut().dispatch_key(chord, surface) {
                Ok(outcome) => outcome,
                Err(error) => {
                    eprintln!("命令执行失败：{error}");
                    return false;
                }
            };

            actions::apply_host_effects(
                outcome.effects,
                &app,
                &workbench,
                &overlays,
                &editor_focus_fallback,
                &panel_runtimes,
                &file_tree,
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

    /// 构造编辑器输入接入 hook：editor_grid 在 paint 阶段拿到 bounds 后调用。
    fn input_handler_hook(&self) -> InputHandlerHook {
        let focus = self.editor_focus.clone();
        let input = self.editor_input.clone();
        Rc::new(move |bounds, window, cx| {
            window.handle_input(&focus, ElementInputHandler::new(bounds, input.clone()), cx);
        })
    }
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut state = self.workbench_state();
        // 光标一移动就重置闪烁为实心，让用户立刻定位到光标；同时重排定时链。
        // 活动编辑器可能是主编辑区，也可能是文件树正在输入名称的内联编辑器。
        let active_cursor = state
            .file_tree
            .pending
            .as_ref()
            .map(|pending| pending.editor.cursor_byte)
            .unwrap_or(state.editor.cursor_byte);
        if self.caret.cursor_moved(active_cursor) {
            let epoch = self.caret.restart();
            self.schedule_caret_blink(epoch, cx);
        }
        state.editor.caret_visible = self.caret.visible();
        let window_controls = self.window_controls_handlers();
        let key_request = self.key_request(KeySurface::Editor);
        let panel_key_request = self.key_request(KeySurface::Panel);
        let file_tree_key_request = self.key_request(KeySurface::FileTree);
        let file_tree_input_handler_hook =
            self.file_tree.input_handler_hook(self.editor_input.clone());
        let file_tree_panel = self.file_tree.panel(
            &state.file_tree,
            &file_tree_key_request,
            &file_tree_input_handler_hook,
            self.caret.visible(),
            window,
        );
        let shortcut_lookup = self.shortcut_lookup();
        let input_handler_hook = self.input_handler_hook();
        let workspace_active = self.overlay_manager.read_with(cx, |manager, _| {
            manager.is_active(OverlayKind::ProjectPicker)
        });
        let language_server_active = self.overlay_manager.read_with(cx, |manager, _| {
            manager.is_active(OverlayKind::LanguageServers)
        });
        let confirm_delete = ConfirmDeleteHandlers {
            confirm: self.bind_action(file_tree_commands::confirm_delete()),
            cancel: self.bind_action(file_tree_commands::cancel_delete()),
        };
        workbench::render(
            &state,
            &self.panel_host,
            Rc::clone(&self.workbench),
            window,
            window_controls,
            self.overlay_shell.clone(),
            self.anchor_registry.clone(),
            workspace_active,
            language_server_active,
            key_request,
            panel_key_request,
            shortcut_lookup,
            input_handler_hook,
            self.editor_focus.clone(),
            self.panel_runtimes.clone(),
            file_tree_panel,
            self.editor_tab_scroll.clone(),
            confirm_delete,
        )
    }
}
