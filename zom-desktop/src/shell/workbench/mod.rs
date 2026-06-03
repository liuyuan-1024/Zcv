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
//! 后两层当前为空 portal，不参与 layout。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Div, Entity, FocusHandle, ScrollHandle, Window, div, prelude::*};

use crate::shell::editor::{EditorSnapshot, TextEditorSlot};
use crate::shell::features::panels::PanelRuntimes;
use crate::shell::features::panels::file_tree::{self, ConfirmDeleteHandlers, FileTreePanel};
use crate::shell::shared::theme::{color, typography};
use crate::shell::{CommandCatalogLookup, CommandTitleLookup, KeyRequest, ShortcutLookup};

mod bars;
pub(crate) mod controller;
pub(crate) mod docks;
pub(crate) mod editor_area;
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
    settings_active: bool,
    language_server_active: bool,
    key_request: KeyRequest,
    shortcut_lookup: ShortcutLookup,
    command_title_lookup: CommandTitleLookup,
    command_catalog_lookup: CommandCatalogLookup,
    editor_slot: Rc<TextEditorSlot>,
    search_query_slot: Rc<TextEditorSlot>,
    search_replacement_slot: Rc<TextEditorSlot>,
    editor_focus: FocusHandle,
    panel_runtimes: PanelRuntimes,
    file_tree: FileTreePanel<'_>,
    editor_tab_scroll: ScrollHandle,
    confirm_delete: ConfirmDeleteHandlers,
    main_editor_snapshot: EditorSnapshot,
) -> Div {
    let dock_resize = dock_resize_request(Rc::clone(&workbench));
    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .bg(color::gray::s02())
        .font(typography::ui_font())
        .text_color(color::gray::s09())
        .child(render_top_bar(
            state,
            window,
            window_controls,
            &shortcut_lookup,
            &command_title_lookup,
            workspace_active,
            settings_active,
        ))
        .child(render_body(
            state,
            host,
            dock_resize,
            key_request,
            editor_slot,
            search_query_slot,
            search_replacement_slot,
            editor_focus,
            panel_runtimes,
            file_tree,
            editor_tab_scroll,
            shortcut_lookup.clone(),
            command_title_lookup.clone(),
            command_catalog_lookup.clone(),
        ))
        .child(render_bottom_bar(
            state,
            &shortcut_lookup,
            &command_title_lookup,
            language_server_active,
            &main_editor_snapshot,
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
    editor_slot: Rc<TextEditorSlot>,
    search_query_slot: Rc<TextEditorSlot>,
    search_replacement_slot: Rc<TextEditorSlot>,
    editor_focus: FocusHandle,
    panel_runtimes: PanelRuntimes,
    file_tree: FileTreePanel<'_>,
    editor_tab_scroll: ScrollHandle,
    shortcut_lookup: ShortcutLookup,
    command_title_lookup: CommandTitleLookup,
    command_catalog_lookup: CommandCatalogLookup,
) -> Div {
    // 所有面板与编辑区共用同一个 KeyRequest —— 角色由 FocusRegistry 在派发瞬间解析。
    // 调用侧不再区分 panel / editor / file_tree 三套闭包。
    //
    // PanelContext 借用此 clone；编辑区那一支后续 move 走 `key_request` 本体。
    // 借用与 move 落到不同 Rc 副本上，互不冲突。
    let key_request_for_panels = Rc::clone(&key_request);
    let panel_ctx = PanelContext {
        has_project: state.has_project,
        file_tree,
        search_state: &state.search,
        search_query_slot: &search_query_slot,
        search_replacement_slot: &search_replacement_slot,
        panel_runtimes: &panel_runtimes,
        key_request: &key_request_for_panels,
        shortcut_lookup: &shortcut_lookup,
        command_title_lookup: &command_title_lookup,
        command_catalog_lookup: &command_catalog_lookup,
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
        editor_slot,
        editor_focus,
        Rc::clone(&dock_resize),
        editor_tab_scroll,
        shortcut_lookup.clone(),
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
