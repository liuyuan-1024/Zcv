//! ProjectSearch —— L3 panel 占位组件。
//!
//! 第一版骨架：渲染「占位中」灰字；P3 接入 zom-engine 搜索能力时再补 UI。

use gpui::Div;

use crate::shell::shared::primitives::panel_placeholder;

pub(crate) const PANEL_ICON: &str = "icons/bottom_bar/project_search.svg";

pub(crate) fn panel_title() -> &'static str {
    "项目搜索"
}

pub(crate) fn render() -> Div {
    panel_placeholder(panel_title(), "项目搜索占位中")
}
