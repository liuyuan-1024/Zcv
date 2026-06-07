//! CenterColumn —— `EditorGrid` + `BottomDock` 的垂直容器
//! （布局模型 4.6 / 手册 20.9）。
//!
//! 与左右 Dock 平级；BottomDock 只占本列下半部，不侵入左右 Dock。
//!
//! 当开启文件级搜索时，标签栏与编辑器之间插入一条内联搜索栏。

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
    // 有打开的文件才显示标签栏；空态不挂这条 sliver。
    if !editor_state.tabs.is_empty() {
        column = column.child(tab_bar::render(
            editor_state,
            tab_scroll,
            &shortcut_lookup,
            &command_title_lookup,
        ));
    }
    // 内联搜索栏：状态由 SearchModel.open 决定，与 dock 无关。
    // 仅在有活动文件时出现——空态打 mod-f 没意义，搜也没东西可搜。
    if search_open && !editor_state.tabs.is_empty() {
        column = column.child(search_runtime.render(
            search_state,
            &key_request,
            &search_query_slot,
            &search_replacement_slot,
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
