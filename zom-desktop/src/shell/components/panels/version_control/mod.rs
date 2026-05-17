//! VersionControl —— L3 panel 占位组件。
//!
//! 第一版骨架：渲染「占位中」灰字。

use gpui::Div;

use crate::shell::components::primitives::panel_placeholder;

pub(crate) const PANEL_ID: &str = "panel.version_control";

#[allow(dead_code)]
pub(crate) const PANEL_ICON: &str = "icons/bottom_bar/version_control.svg";

pub(crate) fn panel_title() -> &'static str {
    "版本管理"
}

pub(crate) fn render() -> Div {
    panel_placeholder(panel_title(), "版本管理占位中")
}
