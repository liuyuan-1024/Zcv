//! CenterColumn —— `EditorGrid` + `BottomDock` 的垂直容器
//! （布局模型 4.6 / 手册 20.9）。
//!
//! 与左右 Dock 平级；BottomDock 只占本列下半部，不侵入左右 Dock。

use gpui::{Div, FocusHandle, ScrollHandle, div, prelude::*};

use crate::shell::workbench::docks::bottom;
use crate::shell::workbench::docks::resize::DockResizeRequest;
use crate::shell::workbench::state::{DockState, EditorState};
use crate::shell::workbench::{PanelContext, PanelHost};
use crate::shell::{CommandTitleLookup, InputHandlerHook, KeyRequest, ShortcutLookup};

mod editor_pane;
mod tab_bar;

pub(crate) fn render(
    bottom_dock_state: &DockState,
    editor_state: &EditorState,
    host: &PanelHost,
    panel_ctx: PanelContext<'_>,
    key_request: KeyRequest,
    input_handler_hook: InputHandlerHook,
    editor_focus: FocusHandle,
    resize: DockResizeRequest,
    tab_scroll: ScrollHandle,
    shortcut_lookup: ShortcutLookup,
    command_title_lookup: CommandTitleLookup,
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
    column = column.child(editor_pane::render(
        editor_state,
        key_request,
        input_handler_hook,
        editor_focus,
    ));
    if bottom_dock_state.is_visible() {
        column = column.child(bottom::render(bottom_dock_state, host, panel_ctx, resize));
    }
    column
}
