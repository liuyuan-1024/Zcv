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
use zom_view::ViewId;

use crate::editor::TextEditorSlot;
use crate::editor::text::EditorSnapshot;
use crate::editor_state::EditorState;
use crate::host_intent::{CommandRequest, KeyRequest, TabCallback};
use crate::shell::bubble::BubbleShell;
use crate::shell::features::panels::PanelRuntimes;
use crate::shell::features::panels::file_tree::{
    self, ConfirmDeleteHandlers, FileTreePanel, FileTreeState,
};
use crate::shell::features::search::{SearchIntentRequest, SearchRuntime, SearchState};
use crate::shell::shared::CommandBinding;
use crate::shell::{CommandCatalogLookup, CommandTitleLookup, ShortcutLookup, focus_request};
use crate::theme::{color, typography};
use crate::ui_id::PanelId;

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
use crate::shell::surfaces::{SurfaceShell, SurfaceStates};
use state::WorkbenchState;

/// 渲染期 feature 状态旁路：
/// workbench 自身只关心布局，三个 feature 视图快照由 view 装配层各自构造后通过本结构传进来。
pub(crate) struct WorkbenchFeatureStates<'a> {
    pub(crate) editor: &'a EditorState,
    pub(crate) file_tree: &'a FileTreeState,
    pub(crate) search: &'a SearchState,
}

#[derive(Clone)]
pub(crate) struct WorkbenchCommandRequests {
    pub(crate) project_picker_open: CommandBinding,
    pub(crate) settings_open: CommandBinding,
    pub(crate) language_servers_open: CommandBinding,
    pub(crate) diagnostics_show_problems: CommandBinding,
    pub(crate) project_search_activate: CommandBinding,
    pub(crate) editor_open_preview: CommandBinding,
    pub(crate) file_search_activate: CommandBinding,
    pub(crate) file_search_dismiss: CommandBinding,
    pub(crate) editor_go_to_line: CommandBinding,
    pub(crate) editor_change_language: CommandBinding,
    pub(crate) panel_toggle: Rc<dyn Fn(PanelId) -> CommandRequest>,
    pub(crate) search_intent: SearchIntentRequest,
    pub(crate) shortcut_lookup: ShortcutLookup,
    pub(crate) title_lookup: CommandTitleLookup,
    pub(crate) tab_select: TabCallback,
    pub(crate) tab_close: Rc<dyn Fn(ViewId) -> CommandBinding>,
}

pub(crate) fn render(
    state: &WorkbenchState,
    features: WorkbenchFeatureStates<'_>,
    commands: WorkbenchCommandRequests,
    host: &PanelHost,
    workbench: Rc<RefCell<WorkbenchController>>,
    window: &Window,
    window_controls: WindowControlsHandlers,
    surface_shell: Entity<SurfaceShell>,
    bubble_shell: Entity<BubbleShell>,
    surfaces: &SurfaceStates,
    key_request: KeyRequest,
    shortcut_lookup: ShortcutLookup,
    command_title_lookup: CommandTitleLookup,
    command_catalog_lookup: CommandCatalogLookup,
    editor_slot: Rc<TextEditorSlot>,
    search_query_slot: Rc<TextEditorSlot>,
    search_replacement_slot: Rc<TextEditorSlot>,
    editor_focus: FocusHandle,
    panel_runtimes: PanelRuntimes,
    search_runtime: SearchRuntime,
    file_tree: FileTreePanel<'_>,
    editor_tab_scroll: ScrollHandle,
    confirm_delete: ConfirmDeleteHandlers,
    main_editor_snapshot: EditorSnapshot,
) -> Div {
    let dock_resize = dock_resize_request(Rc::clone(&workbench));
    let focus_request = focus_request();
    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .bg(color::current().gray.s02)
        .font(typography::ui_font())
        .text_color(color::current().gray.s09)
        .child(render_top_bar(
            state,
            window,
            window_controls,
            &commands,
            surfaces,
        ))
        .child(render_body(
            state,
            &features,
            host,
            dock_resize,
            focus_request,
            key_request,
            editor_slot,
            search_query_slot,
            search_replacement_slot,
            editor_focus,
            panel_runtimes,
            search_runtime,
            file_tree,
            editor_tab_scroll,
            shortcut_lookup.clone(),
            command_title_lookup.clone(),
            command_catalog_lookup.clone(),
            commands.clone(),
        ))
        .child(render_bottom_bar(
            state,
            features.editor,
            &commands,
            &main_editor_snapshot,
            surfaces,
        ))
        .child(surface_shell)
        .child(bubble_shell)
        // 删除确认模态层：处于删除确认态时压在所有面板与 surface 之上。
        .children(file_tree::render_confirm_delete(
            features.file_tree,
            &confirm_delete,
        ))
}

fn render_body(
    state: &WorkbenchState,
    features: &WorkbenchFeatureStates<'_>,
    host: &PanelHost,
    dock_resize: DockResizeRequest,
    focus_request: crate::shell::FocusRequest,
    key_request: KeyRequest,
    editor_slot: Rc<TextEditorSlot>,
    search_query_slot: Rc<TextEditorSlot>,
    search_replacement_slot: Rc<TextEditorSlot>,
    editor_focus: FocusHandle,
    panel_runtimes: PanelRuntimes,
    search_runtime: SearchRuntime,
    file_tree: FileTreePanel<'_>,
    editor_tab_scroll: ScrollHandle,
    shortcut_lookup: ShortcutLookup,
    command_title_lookup: CommandTitleLookup,
    command_catalog_lookup: CommandCatalogLookup,
    commands: WorkbenchCommandRequests,
) -> Div {
    // 所有面板与编辑区共用同一个 KeyRequest —— 角色由当前 AppFocus 在派发瞬间解析。
    //
    // PanelContext 借用此 clone；编辑区那一支后续 move 走 `key_request` 本体。
    // 借用与 move 落到不同 Rc 副本上，互不冲突。
    let key_request_for_panels = Rc::clone(&key_request);
    let panel_ctx = PanelContext {
        has_project: state.has_project,
        file_tree,
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
    let search_open = search_runtime.runtime_handle().is_open();
    row = row.child(editor_area::render(
        &state.bottom_dock,
        features.editor,
        host,
        panel_ctx,
        key_request,
        editor_slot,
        editor_focus,
        focus_request,
        Rc::clone(&dock_resize),
        editor_tab_scroll,
        commands,
        &search_runtime,
        features.search,
        search_query_slot,
        search_replacement_slot,
        search_open,
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
