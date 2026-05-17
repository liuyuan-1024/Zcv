//! CenterColumn —— `EditorGrid` + `BottomDock` 的垂直容器
//! （布局模型 4.6 / 手册 20.9）。
//!
//! 与左右 Dock 平级；BottomDock 只占本列下半部，不侵入左右 Dock。

use gpui::{Div, div, prelude::*};

use crate::shell::layout::DockState;
use crate::shell::panel_host::PanelHost;

use super::{bottom_dock, editor_grid};

pub(crate) fn render(bottom_dock_state: &DockState, host: &PanelHost) -> Div {
    let mut column = div().flex_1().flex().flex_col().h_full();
    column = column.child(editor_grid::render());
    if bottom_dock_state.is_visible() {
        column = column.child(bottom_dock::render(bottom_dock_state, host));
    }
    column
}
