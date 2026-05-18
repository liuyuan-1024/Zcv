//! CenterColumn —— `EditorGrid` + `BottomDock` 的垂直容器
//! （布局模型 4.6 / 手册 20.9）。
//!
//! 与左右 Dock 平级；BottomDock 只占本列下半部，不侵入左右 Dock。

use gpui::{Div, FocusHandle, div, prelude::*};

use crate::shell::model::{DockState, EditorState};
use crate::shell::panels::PanelHost;
use crate::shell::{InputHandlerHook, KeyRequest};

mod editor_grid;

pub(crate) fn render(
    bottom_dock_state: &DockState,
    editor_state: &EditorState,
    host: &PanelHost,
    key_request: KeyRequest,
    input_handler_hook: InputHandlerHook,
    editor_focus: FocusHandle,
) -> Div {
    let mut column = div().flex_1().flex().flex_col().h_full();
    column = column.child(editor_grid::render(
        editor_state,
        key_request,
        input_handler_hook,
        editor_focus,
    ));
    if bottom_dock_state.is_visible() {
        column = column.child(super::dock::bottom::render(bottom_dock_state, host));
    }
    column
}
