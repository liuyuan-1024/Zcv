//! shell 根视图与系统输入法桥接。

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AppContext, Bounds, Context, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    IntoElement, Pixels, Point, Render, UTF16Selection, Window,
};
use zom_command::Invocation;
use zom_command::commands::window as window_commands;

use crate::app::App;

use super::element_ids;
use super::model::WorkbenchState;
use super::overlay::{AnchorRegistry, OverlayAnchor, OverlayKind, OverlayManager, OverlayShell};
use super::panels::PanelHost;
use super::platform::window as platform_window;
use super::platform::window::WindowAction;
use super::workbench;
use super::{ActionRequest, InputHandlerHook, KeyRequest, ShortcutLookup, WindowControlsHandlers};

/// shell 端的根 View：拥有 App 状态与每窗口的 `PanelHost`。
pub(crate) struct ShellView {
    app: Rc<RefCell<App>>,
    panel_host: PanelHost,
    overlay_manager: Entity<OverlayManager>,
    anchor_registry: Entity<AnchorRegistry>,
    overlay_shell: Entity<OverlayShell>,
    editor_focus: FocusHandle,
}

impl ShellView {
    pub(super) fn new(app: App, cx: &mut Context<Self>) -> Self {
        let overlay_manager = cx.new(|_| OverlayManager::new());
        let anchor_registry = cx.new(|_| AnchorRegistry::new());
        let overlay_shell =
            cx.new(|cx| OverlayShell::new(overlay_manager.clone(), anchor_registry.clone(), cx));

        Self {
            app: Rc::new(RefCell::new(app)),
            panel_host: PanelHost::new(),
            overlay_manager,
            anchor_registry,
            overlay_shell,
            editor_focus: cx.focus_handle(),
        }
    }

    pub(super) fn editor_focus(&self) -> FocusHandle {
        self.editor_focus.clone()
    }

    fn workbench_state(&self) -> WorkbenchState {
        self.app.borrow().workbench_state()
    }

    /// 把一个 [`Invocation`] 绑成 [`ActionRequest`]：点击时派发并应用窗口动作。
    fn bind_action(&self, invocation: Invocation) -> ActionRequest {
        let app = Rc::clone(&self.app);
        let overlays = self.overlay_manager.clone();
        let editor_focus_fallback = self.editor_focus.clone();
        Rc::new(move |window, cx| {
            let actions = match app.borrow_mut().dispatch(invocation.clone()) {
                Ok(actions) => actions,
                Err(error) => {
                    eprintln!("命令执行失败：{error}");
                    return;
                }
            };
            apply_window_actions(actions, &overlays, &editor_focus_fallback, window, cx);
        })
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
        let overlays = self.overlay_manager.clone();
        let editor_focus_fallback = self.editor_focus.clone();
        Rc::new(move |chord, window, cx| {
            let outcome = match app.borrow_mut().dispatch_key_input(chord) {
                Ok(outcome) => outcome,
                Err(error) => {
                    eprintln!("命令执行失败：{error}");
                    return false;
                }
            };

            apply_window_actions(
                outcome.actions,
                &overlays,
                &editor_focus_fallback,
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
        let shortcut_lookup = self.shortcut_lookup();
        let input_handler_hook = self.input_handler_hook(cx.entity());
        let workspace_active = self.overlay_manager.read_with(cx, |manager, _| {
            manager.is_active(OverlayKind::ProjectPicker)
        });
        workbench::render(
            &state,
            &self.panel_host,
            window,
            window_controls,
            self.overlay_shell.clone(),
            self.anchor_registry.clone(),
            workspace_active,
            key_request,
            shortcut_lookup,
            input_handler_hook,
            self.editor_focus.clone(),
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

fn apply_window_actions(
    actions: Vec<WindowAction>,
    overlays: &Entity<OverlayManager>,
    editor_focus_fallback: &FocusHandle,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    for action in actions {
        match action {
            WindowAction::OpenOverlay(kind) => {
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
            WindowAction::DismissOverlay => {
                dismiss_overlay(overlays, window, cx);
            }
            other => platform_window::apply(other, window, cx),
        }
    }
}

fn anchor_for_overlay(kind: OverlayKind) -> OverlayAnchor {
    match kind {
        OverlayKind::ProjectPicker => OverlayAnchor::Element(element_ids::TOP_BAR_WORKSPACE.into()),
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
