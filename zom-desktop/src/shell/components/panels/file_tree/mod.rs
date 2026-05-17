//! FileTree —— L3 panel 占位组件。
//!
//! 第一版骨架：渲染「占位中」灰字；后续接入 workspace 文件树时只改本模块。

use gpui::Div;

use crate::shell::components::primitives::panel_placeholder;

pub(crate) const PANEL_ID: &str = "panel.file_tree";

#[allow(dead_code)]
pub(crate) const PANEL_ICON: &str = "icons/bottom_bar/file_tree.svg";

pub(crate) fn panel_title() -> &'static str {
    "文件树"
}

pub(crate) fn render() -> Div {
    panel_placeholder(panel_title(), "文件树占位中")
}
