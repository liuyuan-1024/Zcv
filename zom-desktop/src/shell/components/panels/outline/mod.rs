//! Outline —— L3 panel 占位组件。
//!
//! 第一版骨架：渲染「占位中」灰字；LSP 接入后填充符号大纲。

use gpui::Div;

use crate::shell::components::primitives::panel_placeholder;

pub(crate) const PANEL_ID: &str = "panel.outline";

#[allow(dead_code)]
pub(crate) const PANEL_ICON: &str = "icons/bottom_bar/outline.svg";

pub(crate) fn panel_title() -> &'static str {
    "大纲"
}

pub(crate) fn render() -> Div {
    panel_placeholder(panel_title(), "大纲占位中")
}
