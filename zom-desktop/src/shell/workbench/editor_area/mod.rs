//! CenterColumn —— `EditorGrid` + `BottomDock` 的垂直容器
//! （布局模型 4.6 / 手册 20.9）。
//!
//! 与左右 Dock 平级；BottomDock 只占本列下半部，不侵入左右 Dock。
//!
//! 标签栏与编辑器之间常驻一条 [`FileStatusBar`]：左侧文件路径、右侧动作 glyph。
//! 文件级搜索唤起时，bar 内的右侧动作槽切换为搜索控制组件——不再单独占一条 sliver。

use gpui::{Div, FocusHandle, ScrollHandle, div, prelude::*};

use std::rc::Rc;

use crate::editor::TextEditorSlot;
use crate::editor_state::EditorState;
use crate::shell::features::search::{SearchRuntime, SearchState};
use crate::shell::workbench::docks::bottom;
use crate::shell::workbench::docks::resize::DockResizeRequest;
use crate::shell::workbench::state::DockState;
use crate::shell::workbench::{PanelContext, PanelHost};
use crate::shell::{CommandTitleLookup, KeyRequest, ShortcutLookup};

mod editor_pane;
mod file_status_bar;
mod tab_bar;

#[allow(clippy::too_many_arguments)]
pub(crate) fn render(
    bottom_dock_state: &DockState,
    editor_state: &EditorState,
    host: &PanelHost,
    panel_ctx: PanelContext<'_>,
    key_request: KeyRequest,
    editor_slot: Rc<TextEditorSlot>,
    editor_focus: FocusHandle,
    resize: DockResizeRequest,
    tab_scroll: ScrollHandle,
    shortcut_lookup: ShortcutLookup,
    command_title_lookup: CommandTitleLookup,
    search_runtime: &SearchRuntime,
    search_state: &SearchState,
    search_query_slot: Rc<TextEditorSlot>,
    search_replacement_slot: Rc<TextEditorSlot>,
    search_open: bool,
) -> Div {
    let mut column = div().flex_1().flex().flex_col().h_full().overflow_hidden();
    // 有打开的文件才挂 tab 栏；空态保持空白。
    if !editor_state.tabs.is_empty() {
        column = column.child(tab_bar::render(
            editor_state,
            tab_scroll,
            &shortcut_lookup,
            &command_title_lookup,
        ));
    }
    // 文件状态栏：仅在有活动文件时出现；搜索打开时由它在内部追加搜索第二行。
    if let Some(active) = editor_state.tabs.iter().find(|tab| tab.is_active) {
        column = column.child(file_status_bar::render(
            active,
            &key_request,
            search_runtime,
            search_state,
            &search_query_slot,
            &search_replacement_slot,
            search_open,
            &shortcut_lookup,
            &command_title_lookup,
        ));
    }
    column = column.child(editor_pane::render(
        editor_state,
        key_request,
        editor_slot,
        editor_focus,
    ));
    if bottom_dock_state.is_visible() {
        column = column.child(bottom::render(bottom_dock_state, host, panel_ctx, resize));
    }
    column
}
