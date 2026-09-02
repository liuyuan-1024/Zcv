use std::path::PathBuf;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, Render, TestAppContext, Window, div,
    prelude::*,
};
use zcv_text::SearchQuery;
use zcv_workspace::{
    Direction, Item, ItemHandle, SearchEvent, SearchableItem, SearchableItemHandle,
    ToolbarItemLocation, ToolbarItemView,
};

use crate::buffer_search::BufferSearchButton;

struct TestView;

impl Render for TestView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

struct TestItem {
    focus: FocusHandle,
    path: Option<PathBuf>,
    exposes_search: bool,
}

impl EventEmitter<SearchEvent> for TestItem {}

impl Focusable for TestItem {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TestItem {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Item for TestItem {
    type Event = SearchEvent;

    fn tab_content_text(&self, _cx: &App) -> gpui::SharedString {
        "测试文档".into()
    }

    fn item_path(&self, _cx: &App) -> Option<PathBuf> {
        self.path.clone()
    }

    fn as_searchable(
        &self,
        self_handle: &gpui::Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn SearchableItemHandle>> {
        self.exposes_search
            .then(|| Box::new(self_handle.clone()) as Box<dyn SearchableItemHandle>)
    }
}

impl SearchableItem for TestItem {
    fn search(&mut self, _query: &SearchQuery, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn clear_search(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn search_count(&self, _cx: &App) -> (usize, Option<usize>) {
        (0, None)
    }

    fn activate_match_in_direction(
        &mut self,
        _direction: Direction,
        _count: usize,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn replace_current(
        &mut self,
        _replacement: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        false
    }

    fn replace_all(
        &mut self,
        _replacement: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> usize {
        0
    }
}

#[gpui::test]
fn search_button_does_not_require_an_active_path(cx: &mut TestAppContext) {
    cx.add_window_view(|window, cx| {
        let item = cx.new(|cx| TestItem {
            focus: cx.focus_handle(),
            path: None,
            exposes_search: true,
        });
        let button = cx.new(|_| BufferSearchButton);

        let location = button.update(cx, |button, cx| {
            button.set_active_pane_item(Some(&item as &dyn ItemHandle), window, cx)
        });

        assert_eq!(item.read(cx).active_path(cx), None);
        assert_eq!(location, ToolbarItemLocation::PrimaryRight);
        TestView
    });
}

#[gpui::test]
fn search_button_does_not_use_a_path_as_search_capability(cx: &mut TestAppContext) {
    cx.add_window_view(|window, cx| {
        let item = cx.new(|cx| TestItem {
            focus: cx.focus_handle(),
            path: Some(PathBuf::from("notes.txt")),
            exposes_search: false,
        });
        let button = cx.new(|_| BufferSearchButton);

        let location = button.update(cx, |button, cx| {
            button.set_active_pane_item(Some(&item as &dyn ItemHandle), window, cx)
        });

        assert_eq!(location, ToolbarItemLocation::Hidden);
        TestView
    });
}
