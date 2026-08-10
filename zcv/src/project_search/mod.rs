//! SearchButton —— 底栏项目搜索按钮。
//!
//! 对标 Zed 的 `SearchButton`。当前为占位，后续接入搜索状态。

use gpui::{Context, Render, Window, prelude::*};

use crate::workspace::StatusItemView;
use crate::workspace::ToggleProjectSearch;
use zcv_ui::Glyph;
use zcv_workspace::ItemHandle;

pub(crate) struct ProjectSearchButton;

impl ProjectSearchButton {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl StatusItemView for ProjectSearchButton {
    fn set_active_pane_item(&mut self, _item: Option<&dyn ItemHandle>, _cx: &mut Context<Self>) {}
}

impl Render for ProjectSearchButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        Glyph::icon("search-button", "icons/magnifying_glass.svg")
            .label("项目搜索")
            .shortcut(&ToggleProjectSearch, cx)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleProjectSearch), cx))
            .into_any_element()
    }
}
