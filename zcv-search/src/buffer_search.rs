//! 文件内搜索协调器。
//!
//! 搜索入口与 BufferSearchBar 共同跟随 Pane 中的普通可搜索 Item。

use gpui::{App, AppContext, Context, Entity, EventEmitter, Render, Window, div, prelude::*};
use zcv_actions::DeploySearch;
use zcv_ui::Button;
use zcv_workspace::{
    ItemHandle, ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView, Workspace,
};

use crate::{project_search, search_bar::SearchBar};

fn accepts_buffer_search_item(item: Option<&dyn ItemHandle>, cx: &App) -> bool {
    item.is_some_and(|item| {
        item.as_searchable(cx).is_some() && !project_search::is_project_search_item(item, cx)
    })
}

/// 文件内搜索入口；是否显示只由 Item 的搜索能力决定。
pub(super) struct BufferSearchButton;

impl EventEmitter<ToolbarItemEvent> for BufferSearchButton {}

impl ToolbarItemView for BufferSearchButton {
    fn set_active_pane_item(
        &mut self,
        active_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        if accepts_buffer_search_item(active_item, cx) {
            ToolbarItemLocation::PrimaryRight
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

impl Render for BufferSearchButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        Button::icon("toolbar-file-search", "icons/magnifying_glass.svg")
            .label("搜索")
            .shortcut(&DeploySearch, cx)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(DeploySearch), cx))
    }
}

pub(super) struct BufferSearchBar {
    search_bar: Entity<SearchBar>,
}

impl BufferSearchBar {
    fn new(search_bar: Entity<SearchBar>, cx: &mut Context<Self>) -> Self {
        cx.subscribe(&search_bar, |_, _, event: &ToolbarItemEvent, cx| {
            cx.emit(*event)
        })
        .detach();
        Self { search_bar }
    }

    pub(super) fn deploy(
        &mut self,
        query_seed: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.deploy(query_seed, window, cx)
        });
    }
}

impl EventEmitter<ToolbarItemEvent> for BufferSearchBar {}

impl ToolbarItemView for BufferSearchBar {
    fn set_active_pane_item(
        &mut self,
        active_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        if !accepts_buffer_search_item(active_item, cx) {
            return ToolbarItemLocation::Hidden;
        }
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.set_active_item(active_item, window, cx);
            search_bar.location()
        })
    }
}

impl Render for BufferSearchBar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(self.search_bar.clone())
    }
}

pub(super) fn install(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<BufferSearchBar> {
    let search_bar = cx.new(|cx| SearchBar::new("BufferSearchBar", cx));
    let buffer_search_bar = cx.new(|cx| BufferSearchBar::new(search_bar, cx));
    let toolbar = workspace.pane().read(cx).toolbar().clone();
    toolbar.update(cx, |toolbar, cx| {
        toolbar.add_item(cx.new(|_| BufferSearchButton), window, cx);
        toolbar.add_item(buffer_search_bar.clone(), window, cx);
    });
    buffer_search_bar
}
