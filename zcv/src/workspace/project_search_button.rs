//! SearchButton —— 底栏项目搜索按钮。
//!
//! 对标 Zed 的 `SearchButton`。当前为占位，后续接入搜索状态。

use gpui::{Context, Entity, Render, Window, prelude::*};

use crate::editor::editor::Editor;
use crate::ui::glyph::Glyph;
use crate::workspace::dock::ToggleProjectSearch;
use crate::workspace::status_bar::StatusItemView;

pub(crate) struct ProjectSearchButton;

impl ProjectSearchButton {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl StatusItemView for ProjectSearchButton {
    fn set_active_editor(&mut self, _editor: Option<&Entity<Editor>>, _cx: &mut Context<Self>) {}
}

impl Render for ProjectSearchButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        Glyph::icon("search-button", "icons/panels/search.svg")
            .label("项目搜索")
            .shortcut_by_name("dock::ToggleProjectSearch", cx)
            .on_click(|window, cx| window.dispatch_action(Box::new(ToggleProjectSearch), cx))
            .into_any_element()
    }
}
