//! Debug —— L3 panel 占位组件。
//!
//! 第一版骨架：渲染「占位中」灰字。

use gpui::Div;

use crate::shell::shared::primitives::panel_placeholder;

pub(crate) const PANEL_ICON: &str = "icons/bottom_bar/debug.svg";

pub(crate) fn render() -> Div {
    panel_placeholder("调试占位中")
}
