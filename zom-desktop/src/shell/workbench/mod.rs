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
//!   [20] SurfaceShell
//!   [30] BubbleShell
//! 后两层骨架阶段为空 portal，不参与 layout。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Div, Entity, FocusHandle, ScrollHandle, Window, div, prelude::*};

use crate::shell::features::PanelRuntimes;
use crate::shell::features::file_tree::{self, ConfirmDeleteHandlers, FileTreePanel};
use crate::shell::shared::theme::{color, radius};
use crate::shell::{CommandTitleLookup, InputHandlerHook, KeyRequest, ShortcutLookup};

mod bars;
pub(crate) mod controller;
pub(crate) mod docks;
mod editor_area;
pub(crate) mod state;

pub(crate) use self::bars::WindowControlsHandlers;
use self::bars::{render_bottom_bar, render_top_bar};
use self::controller::WorkbenchController;
use self::docks::resize::{DockResizeBounds, DockResizeEvent, DockResizeRequest};
pub(crate) use self::docks::{PanelContext, PanelHost};
use crate::shell::surfaces::SurfaceShell;
use state::WorkbenchState;

pub(crate) fn render(
    state: &WorkbenchState,
    host: &PanelHost,
    workbench: Rc<RefCell<WorkbenchController>>,
    window: &Window,
    window_controls: WindowControlsHandlers,
    surface_shell: Entity<SurfaceShell>,
    workspace_active: bool,
    language_server_active: bool,
    key_request: KeyRequest,
    panel_key_request: KeyRequest,
    shortcut_lookup: ShortcutLookup,
    command_title_lookup: CommandTitleLookup,
    input_handler_hook: InputHandlerHook,
    editor_focus: FocusHandle,
    panel_runtimes: PanelRuntimes,
    file_tree: FileTreePanel<'_>,
    editor_tab_scroll: ScrollHandle,
    confirm_delete: ConfirmDeleteHandlers,
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
        .child(render_top_bar(
            state,
            window,
            window_controls,
            &shortcut_lookup,
            &command_title_lookup,
            workspace_active,
        ))
        .child(render_body(
            state,
            host,
            dock_resize,
            key_request,
            panel_key_request,
            input_handler_hook,
            editor_focus,
            panel_runtimes,
            file_tree,
            editor_tab_scroll,
            shortcut_lookup.clone(),
            command_title_lookup.clone(),
        ))
        .child(render_bottom_bar(
            state,
            &shortcut_lookup,
            &command_title_lookup,
            language_server_active,
        ))
        .child(surface_shell)
        .child(crate::shell::bubble::render())
        // 删除确认模态层：处于删除确认态时压在所有面板与 surface 之上。
        .children(file_tree::render_confirm_delete(
            &state.file_tree,
            &confirm_delete,
        ))
}

fn render_body(
    state: &WorkbenchState,
    host: &PanelHost,
    dock_resize: DockResizeRequest,
    key_request: KeyRequest,
    panel_key_request: KeyRequest,
    input_handler_hook: InputHandlerHook,
    editor_focus: FocusHandle,
    panel_runtimes: PanelRuntimes,
    file_tree: FileTreePanel<'_>,
    editor_tab_scroll: ScrollHandle,
    shortcut_lookup: ShortcutLookup,
    command_title_lookup: CommandTitleLookup,
) -> Div {
    let panel_ctx = PanelContext {
        has_project: state.has_project,
        file_tree,
        panel_runtimes: &panel_runtimes,
        panel_key_request: &panel_key_request,
        command_title_lookup: &command_title_lookup,
    };
    let mut row = div().flex_1().flex().flex_row().w_full().overflow_hidden();

    if state.left_dock.is_visible() {
        row = row.child(docks::left::render(
            &state.left_dock,
            host,
            panel_ctx,
            Rc::clone(&dock_resize),
        ));
    }
    row = row.child(editor_area::render(
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
        Rc::clone(&command_title_lookup),
    ));
    if state.right_dock.is_visible() {
        row = row.child(docks::right::render(
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
        let bounds = DockResizeBounds::from_viewport(viewport_size.width, viewport_size.height);
        let dragging = matches!(event, DockResizeEvent::Drag { .. });
        workbench.borrow_mut().handle_dock_resize(event, bounds);
        if dragging {
            window.refresh();
        }
    })
}
