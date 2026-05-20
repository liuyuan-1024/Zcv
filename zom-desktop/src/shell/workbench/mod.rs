//! WorkbenchFrame —— 窗口顶层装配（布局模型 4 / 手册 二十一 z-index 分层）。
//!
//! ```text
//! ┌────────────────────────────────────┐
//! │ TopBar                             │
//! ├───────┬────────────────────┬───────┤
//! │ Left  │    EditorGrid      │ Right │
//! │ Dock  ├────────────────────┤ Dock  │
//! │       │    BottomDock      │       │
//! ├───────┴────────────────────┴───────┤
//! │ BottomBar                          │
//! └────────────────────────────────────┘
//! ```
//!
//! 渲染层次（手册 21.2）：
//!   [10] WorkbenchFrame
//!   [20] OverlayShell
//!   [30] BubbleShell
//! 后两层骨架阶段为空 portal，不参与 layout。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Div, Entity, FocusHandle, ScrollHandle, Window, div, prelude::*};

use crate::shell::features::file_tree::FileTreePanel;
use crate::shell::shared::theme::{color, radius};
use crate::shell::{InputHandlerHook, KeyRequest, ShortcutLookup, WindowControlsHandlers};

pub(crate) mod controller;
pub(crate) mod dock_resize;
pub(crate) mod overlay;
mod panel_host;
mod regions;
pub(crate) mod state;

use self::controller::WorkbenchController;
use self::dock_resize::{DockResizeBounds, DockResizeEvent, DockResizeRequest};
use self::overlay::{AnchorRegistry, OverlayShell};
pub(crate) use self::panel_host::{PanelContext, PanelHost};
use state::WorkbenchState;

pub(crate) fn render(
    state: &WorkbenchState,
    host: &PanelHost,
    workbench: Rc<RefCell<WorkbenchController>>,
    window: &Window,
    window_controls: WindowControlsHandlers,
    overlay_shell: Entity<OverlayShell>,
    anchor_registry: Entity<AnchorRegistry>,
    workspace_active: bool,
    language_server_active: bool,
    key_request: KeyRequest,
    shortcut_lookup: ShortcutLookup,
    input_handler_hook: InputHandlerHook,
    editor_focus: FocusHandle,
    file_tree: FileTreePanel<'_>,
    editor_tab_scroll: ScrollHandle,
) -> Div {
    let dock_resize = dock_resize_request(Rc::clone(&workbench));
    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .rounded(radius::window())
        .border_1()
        .border_color(color::gray::g40())
        .bg(color::gray::g05())
        .text_color(color::gray::g90())
        .child(regions::top_bar::render(
            state,
            window,
            window_controls,
            &shortcut_lookup,
            anchor_registry.clone(),
            workspace_active,
        ))
        .child(render_body(
            state,
            host,
            dock_resize,
            key_request,
            input_handler_hook,
            editor_focus,
            file_tree,
            editor_tab_scroll,
            shortcut_lookup.clone(),
        ))
        .child(regions::bottom_bar::render(
            state,
            &shortcut_lookup,
            anchor_registry,
            language_server_active,
        ))
        .child(overlay_shell)
        .child(regions::overlay_layer::bubble_layer::render())
}

fn render_body(
    state: &WorkbenchState,
    host: &PanelHost,
    dock_resize: DockResizeRequest,
    key_request: KeyRequest,
    input_handler_hook: InputHandlerHook,
    editor_focus: FocusHandle,
    file_tree: FileTreePanel<'_>,
    editor_tab_scroll: ScrollHandle,
    shortcut_lookup: ShortcutLookup,
) -> Div {
    let panel_ctx = PanelContext {
        has_project: state.has_project,
        file_tree,
    };
    let mut row = div().flex_1().flex().flex_row().w_full().overflow_hidden();

    if state.left_dock.is_visible() {
        row = row.child(regions::left_dock::render(
            &state.left_dock,
            host,
            panel_ctx,
            Rc::clone(&dock_resize),
        ));
    }
    row = row.child(regions::editor_area::render(
        &state.bottom_dock,
        &state.editor,
        host,
        panel_ctx,
        key_request,
        input_handler_hook,
        editor_focus,
        Rc::clone(&dock_resize),
        editor_tab_scroll,
        shortcut_lookup,
    ));
    if state.right_dock.is_visible() {
        row = row.child(regions::right_dock::render(
            &state.right_dock,
            host,
            panel_ctx,
            Rc::clone(&dock_resize),
        ));
    }

    row
}

fn dock_resize_request(workbench: Rc<RefCell<WorkbenchController>>) -> DockResizeRequest {
    Rc::new(move |event, window, _cx| {
        let viewport_size = window.viewport_size();
        let bounds = DockResizeBounds::from_viewport(viewport_size.width);
        let dragging = matches!(event, DockResizeEvent::Drag { .. });
        workbench.borrow_mut().handle_dock_resize(event, bounds);
        if dragging {
            window.refresh();
        }
    })
}
