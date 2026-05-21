//! shell 根视图与系统输入法桥接。

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AppContext, Bounds, Context, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    IntoElement, Pixels, Point, Render, ScrollHandle, UTF16Selection, Window,
};
use zom_command::HostEffect;
use zom_command::Invocation;
use zom_command::commands::window as window_commands;

use crate::app::App;

use super::features::PanelId;
use super::features::file_tree::FileTreeRuntime;
use super::platform::project as platform_project;
use super::platform::window as platform_window;
use super::shared::element_ids;
use super::workbench;
use super::workbench::PanelHost;
use super::workbench::controller::WorkbenchController;
use super::workbench::overlay::{
    AnchorRegistry, OverlayAnchor, OverlayKind, OverlayManager, OverlayShell,
};
use super::workbench::state::WorkbenchState;
use super::{ActionRequest, InputHandlerHook, KeyRequest, ShortcutLookup, WindowControlsHandlers};

/// shell 端的根 View：拥有 App 状态与每窗口的 `PanelHost`。
pub(crate) struct ShellView {
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    panel_host: PanelHost,
    overlay_manager: Entity<OverlayManager>,
    anchor_registry: Entity<AnchorRegistry>,
    overlay_shell: Entity<OverlayShell>,
    editor_focus: FocusHandle,
    file_tree: FileTreeRuntime,
    /// 编辑区标签栏的滚动状态。跨帧保留，否则每帧重建会丢失滚动位置。
    editor_tab_scroll: ScrollHandle,
}

impl ShellView {
    pub(super) fn new(app: App, cx: &mut Context<Self>) -> Self {
        let app = Rc::new(RefCell::new(app));
        let workbench = Rc::new(RefCell::new(WorkbenchController::new()));
        let overlay_manager = cx.new(|_| OverlayManager::new());
        let anchor_registry = cx.new(|_| AnchorRegistry::new());
        let editor_focus = cx.focus_handle();
        let file_tree = FileTreeRuntime::new(cx);
        let open_local_project = bind_action_request(
            Rc::clone(&app),
            Rc::clone(&workbench),
            overlay_manager.clone(),
            editor_focus.clone(),
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
            editor_focus,
            file_tree,
            editor_tab_scroll: ScrollHandle::new(),
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
    }

    /// 打开指定路径的本地项目（不弹选择器）。开发阶段默认项目经由此入口，
    /// 与选择器流程汇聚到同一个 [`apply_project_open`]。
    #[cfg(debug_assertions)]
    pub(super) fn open_project(&self, project_root: std::path::PathBuf, window: &mut Window) {
        apply_project_open(
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
        bind_action_request(
            Rc::clone(&self.app),
            Rc::clone(&self.workbench),
            self.overlay_manager.clone(),
            self.editor_focus.clone(),
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

    fn key_request(&self) -> KeyRequest {
        let app = Rc::clone(&self.app);
        let workbench = Rc::clone(&self.workbench);
        let overlays = self.overlay_manager.clone();
        let editor_focus_fallback = self.editor_focus.clone();
        let file_tree = self.file_tree.clone();
        Rc::new(move |chord, window, cx| {
            let outcome = match app.borrow_mut().dispatch_key_input(chord) {
                Ok(outcome) => outcome,
                Err(error) => {
                    eprintln!("命令执行失败：{error}");
                    return false;
                }
            };

            apply_host_effects(
                outcome.effects,
                &app,
                &workbench,
                &overlays,
                &editor_focus_fallback,
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

    /// 构造 IME 接入 hook：editor_grid 在 paint 阶段拿到 bounds 后调用。
    fn input_handler_hook(&self, entity: Entity<ShellView>) -> InputHandlerHook {
        let focus = self.editor_focus.clone();
        Rc::new(move |bounds, window, cx| {
            window.handle_input(&focus, ElementInputHandler::new(bounds, entity.clone()), cx);
        })
    }
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.workbench_state();
        let window_controls = self.window_controls_handlers();
        let key_request = self.key_request();
        let file_tree_key_request = self.file_tree.key_request(
            Rc::clone(&self.app),
            self.editor_focus.clone(),
            self.key_request(),
        );
        let file_tree_panel =
            self.file_tree
                .panel(&state.file_tree, &file_tree_key_request, window);
        let shortcut_lookup = self.shortcut_lookup();
        let input_handler_hook = self.input_handler_hook(cx.entity());
        let workspace_active = self.overlay_manager.read_with(cx, |manager, _| {
            manager.is_active(OverlayKind::ProjectPicker)
        });
        let language_server_active = self.overlay_manager.read_with(cx, |manager, _| {
            manager.is_active(OverlayKind::LanguageServers)
        });
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
            shortcut_lookup,
            input_handler_hook,
            self.editor_focus.clone(),
            file_tree_panel,
            self.editor_tab_scroll.clone(),
        )
    }
}

fn dismiss_overlay(overlays: &Entity<OverlayManager>, window: &mut Window, cx: &mut gpui::App) {
    let Some(focus_to_restore) = overlays.update(cx, |overlays, cx| overlays.dismiss(cx)) else {
        return;
    };
    window.focus(&focus_to_restore);
    window.refresh();
}

fn bind_action_request(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    overlays: Entity<OverlayManager>,
    editor_focus_fallback: FocusHandle,
    file_tree: FileTreeRuntime,
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
            &overlays,
            &editor_focus_fallback,
            &file_tree,
            window,
            cx,
        );
    })
}

fn apply_host_effects(
    effects: Vec<HostEffect>,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    overlays: &Entity<OverlayManager>,
    editor_focus_fallback: &FocusHandle,
    file_tree: &FileTreeRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) {
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
                if panel == PanelId::FileTree {
                    file_tree.handle_toggle_request(workbench, editor_focus_fallback, window);
                } else {
                    workbench.borrow_mut().toggle_panel(panel);
                    window.refresh();
                }
            }
            HostEffect::ShowProjectPicker => {
                open_overlay(
                    OverlayKind::ProjectPicker,
                    overlays,
                    editor_focus_fallback,
                    window,
                    cx,
                );
            }
            HostEffect::OpenLocalProject => {
                open_local_project(
                    Rc::clone(app),
                    Rc::clone(workbench),
                    overlays,
                    file_tree.clone(),
                    window,
                    cx,
                );
            }
            HostEffect::ShowLanguageServers => {
                open_overlay(
                    OverlayKind::LanguageServers,
                    overlays,
                    editor_focus_fallback,
                    window,
                    cx,
                );
            }
            HostEffect::DismissOverlay => dismiss_overlay(overlays, window, cx),
        }
    }
}

fn open_overlay(
    kind: OverlayKind,
    overlays: &Entity<OverlayManager>,
    editor_focus_fallback: &FocusHandle,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let anchor = anchor_for_overlay(kind);
    // 手册 21.7：关闭时焦点回到"先前 focus 目标"——open 这一帧 window
    // 里实际聚焦的元素。查不到（窗口刚启动等）退回 editor 焦点，避免
    // 关闭后焦点悬空。
    let focus_to_restore = window
        .focused(cx)
        .unwrap_or_else(|| editor_focus_fallback.clone());
    overlays.update(cx, |overlays, cx| {
        overlays.open(kind, anchor, focus_to_restore, cx);
    });
    window.refresh();
}

fn open_local_project(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    overlays: &Entity<OverlayManager>,
    file_tree: FileTreeRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    dismiss_overlay(overlays, window, cx);
    let selection = platform_project::prompt_for_local_project(cx);
    window
        .spawn(cx, async move |cx| {
            let Some(project_root) = selection.await else {
                return;
            };
            if let Err(error) = cx.update(|window, _| {
                apply_project_open(&app, &workbench, &file_tree, project_root, window);
            }) {
                eprintln!("打开本地项目失败：{error}");
            }
        })
        .detach();
}

/// 打开本地项目的统一落点：更新 `App` 状态、展开并聚焦文件树、刷新窗口。
/// 选择器流程与开发阶段默认项目都经由此函数，保证两条路径行为一致。
fn apply_project_open(
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    file_tree: &FileTreeRuntime,
    project_root: std::path::PathBuf,
    window: &mut Window,
) {
    app.borrow_mut().open_local_project(project_root);
    file_tree.reveal_after_project_open(app, workbench, window);
    window.refresh();
}

fn anchor_for_overlay(kind: OverlayKind) -> OverlayAnchor {
    match kind {
        OverlayKind::ProjectPicker => OverlayAnchor::Element(element_ids::TOP_BAR_WORKSPACE.into()),
        OverlayKind::LanguageServers => {
            OverlayAnchor::Element(element_ids::BOTTOM_BAR_LANGUAGE_SERVER.into())
        }
    }
}

// ===== EntityInputHandler：把 macOS NSTextInputClient 接到引擎组合输入流程 =====
//
// 这里只做薄薄一层胶水：所有坐标换算、组合输入状态、selection 维护都落到 App
// 与 zom-engine。换其它平台时（Wayland / IBus 等）只需要把对接面替换，逻辑层无需动。

impl EntityInputHandler for ShellView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        self.app.borrow().ime_text_for_range_utf16(range)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.app
            .borrow()
            .ime_selected_range_utf16()
            .map(|(range, reversed)| UTF16Selection { range, reversed })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.app.borrow().ime_marked_range_utf16()
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.app.borrow_mut().ime_unmark() {
            eprintln!("IME unmark 失败：{error}");
        }
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.app.borrow_mut().ime_replace_text(range, text) {
            eprintln!("IME replace_text 失败：{error}");
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) =
            self.app
                .borrow_mut()
                .ime_replace_and_mark_text(range, new_text, new_selected_range)
        {
            eprintln!("IME replace_and_mark_text 失败：{error}");
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // 第一版：候选窗就贴在编辑区左上角；后续接入光标坐标再精修。
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
