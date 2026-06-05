//! BottomDock —— 中间列底部停靠区 L4 Region（布局模型 4.5 / 手册 20.9）。
//!
//! 只位于 `EditorGrid` 下方，水平边界与 EditorGrid 对齐；不侵入左右 Dock。

use gpui::{Div, div, prelude::*};

use crate::shell::workbench::state::{DockAreaId, DockState};
use crate::shell::workbench::{PanelContext, PanelHost};
use crate::theme::{color, typography};
use crate::ui_id::PanelId;

use super::resize::{self, DockResizeRequest};
use super::{DockEdge, dock_frame};

pub(in crate::shell::workbench) const PANELS: &[PanelId] = &[PanelId::Terminal, PanelId::Debug];

pub(crate) fn render(
    state: &DockState,
    host: &PanelHost,
    ctx: PanelContext<'_>,
    resize: DockResizeRequest,
) -> Div {
    let body = match state.active_panel() {
        Some(id) => host.render(id, ctx),
        None => empty_body().into_any_element(),
    };

    dock_frame(DockEdge::Bottom)
        .h(state.size)
        .child(div().flex_1().child(body))
        .child(resize::render_handle(DockAreaId::Bottom, resize))
}

fn empty_body() -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size_full()
        .text_size(typography::ui())
        .text_color(color::gray::s08())
}
