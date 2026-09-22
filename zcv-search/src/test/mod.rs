use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, Render, TestAppContext, Window, div,
    prelude::*,
};
use zcv_language::LanguageRegistry;
use zcv_project::{Project, SearchQuery};
use zcv_workspace::{
    Breadcrumbs, Direction, Item, ItemHandle, Pane, PreviewButton, SearchEvent, SearchableItem,
    SearchableItemHandle, ToolbarItemLocation, ToolbarItemView,
};

use zcv_editor::Editor;

use crate::buffer_search::DocumentToolbar;

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
    last_query: Option<String>,
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
    fn search(&mut self, query: &SearchQuery, _window: &mut Window, _cx: &mut Context<Self>) {
        self.last_query = Some(query.query.clone());
    }

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

/// 组合 Item：通过 `act_as_type` 暴露内层编辑器，但自身不是编辑器。
struct CompositeItem {
    focus: FocusHandle,
    inner_editor: gpui::Entity<Editor>,
}

impl EventEmitter<SearchEvent> for CompositeItem {}

impl Focusable for CompositeItem {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CompositeItem {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Item for CompositeItem {
    type Event = SearchEvent;

    fn tab_content_text(&self, _cx: &App) -> gpui::SharedString {
        "组合文档".into()
    }

    fn as_searchable(
        &self,
        _self_handle: &gpui::Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn SearchableItemHandle>> {
        Some(Box::new(self.inner_editor.clone()))
    }

    fn act_as_type(
        &self,
        type_id: std::any::TypeId,
        self_handle: &gpui::Entity<Self>,
        _cx: &App,
    ) -> Option<gpui::AnyEntity> {
        if type_id == std::any::TypeId::of::<Self>() {
            Some(self_handle.clone().into())
        } else if type_id == std::any::TypeId::of::<Editor>() {
            Some(self.inner_editor.clone().into())
        } else {
            None
        }
    }
}

fn document_toolbar(cx: &mut TestAppContext) -> gpui::Entity<DocumentToolbar> {
    let pane = cx.new(Pane::new);
    let preview_button = cx.new(|_| PreviewButton::new(pane.downgrade()));
    let project =
        cx.new(|cx| Project::new(PathBuf::from("."), Arc::new(LanguageRegistry::new()), cx));
    let language_registry = cx.read_entity(&project, |project, _| project.language_registry());
    let breadcrumbs = cx.new(|_| Breadcrumbs::new(project));
    cx.new(|cx| DocumentToolbar::new(preview_button, breadcrumbs, language_registry, cx))
}

#[gpui::test]
fn buffer_search_does_not_require_an_active_path(cx: &mut TestAppContext) {
    let bar = document_toolbar(cx);
    cx.add_window_view(|window, cx| {
        let item = cx.new(|cx| TestItem {
            focus: cx.focus_handle(),
            path: None,
            exposes_search: true,
            last_query: None,
        });
        bar.update(cx, |bar, cx| {
            bar.set_active_pane_item(Some(&item as &dyn ItemHandle), window, cx)
        });

        assert_eq!(item.read(cx).active_path(cx), None);
        TestView
    });
}

#[gpui::test]
fn buffer_search_does_not_use_a_path_as_search_capability(cx: &mut TestAppContext) {
    let bar = document_toolbar(cx);
    cx.add_window_view(|window, cx| {
        let item = cx.new(|cx| TestItem {
            focus: cx.focus_handle(),
            path: Some(PathBuf::from("notes.txt")),
            exposes_search: false,
            last_query: None,
        });
        bar.update(cx, |bar, cx| {
            bar.set_active_pane_item(Some(&item as &dyn ItemHandle), window, cx)
        });
        TestView
    });
}

/// 编辑器显示缓冲搜索工具区；其他 Item 由各自的工具项承担，工具区隐藏。
#[gpui::test]
fn document_toolbar_is_visible_for_editor_and_hidden_for_other_items(cx: &mut TestAppContext) {
    let bar = document_toolbar(cx);
    cx.add_window_view(|window, cx| {
        let editor =
            cx.new(|cx| Editor::single_line(std::sync::Arc::new(LanguageRegistry::new()), cx));
        let editor_location = bar.update(cx, |bar, cx| {
            bar.set_active_pane_item(Some(&editor as &dyn ItemHandle), window, cx)
        });
        assert_eq!(editor_location, ToolbarItemLocation::Secondary);

        let other = cx.new(|cx| TestItem {
            focus: cx.focus_handle(),
            path: None,
            exposes_search: true,
            last_query: None,
        });
        let other_location = bar.update(cx, |bar, cx| {
            bar.set_active_pane_item(Some(&other as &dyn ItemHandle), window, cx)
        });
        assert_eq!(other_location, ToolbarItemLocation::Hidden);
        TestView
    });
}

/// 组合 Item 仅暴露内层编辑器时，编辑器搜索工具区必须隐藏（差异/提交图等由各自工具项承担搜索）。
#[gpui::test]
fn document_toolbar_is_hidden_for_composite_items_that_expose_an_editor(cx: &mut TestAppContext) {
    let bar = document_toolbar(cx);
    cx.add_window_view(|window, cx| {
        let inner_editor =
            cx.new(|cx| Editor::single_line(std::sync::Arc::new(LanguageRegistry::new()), cx));
        let composite = cx.new(|cx| CompositeItem {
            focus: cx.focus_handle(),
            inner_editor,
        });
        let location = bar.update(cx, |bar, cx| {
            bar.set_active_pane_item(Some(&composite as &dyn ItemHandle), window, cx)
        });
        assert_eq!(
            location,
            ToolbarItemLocation::Hidden,
            "组合 Item 暴露内层编辑器时不应再显示编辑器搜索工具区"
        );
        TestView
    });
}

/// 项目搜索的结果编辑器同样通过 Item 协议暴露为编辑器；
/// 通用文档工具栏只服务本身就是编辑器的 Item，因此该视图的工具区由自身承担。
#[gpui::test]
fn project_search_view_acts_as_editor_and_owns_its_toolbar(cx: &mut TestAppContext) {
    let project =
        cx.new(|cx| Project::new(PathBuf::from("."), Arc::new(LanguageRegistry::new()), cx));
    let view = cx.new(|cx| crate::project_search::ProjectSearchView::new(project, cx));
    let bar = document_toolbar(cx);
    cx.add_window_view(|window, cx| {
        let handle: &dyn ItemHandle = &view;
        assert!(
            handle.act_as::<Editor>(cx).is_some(),
            "搜索结果同样是编辑器，应通过 act_as_type 暴露结果编辑器"
        );
        assert!(handle.as_searchable(cx).is_some());
        let location = bar.update(cx, |bar, cx| {
            bar.set_active_pane_item(Some(handle), window, cx)
        });
        assert_eq!(
            location,
            ToolbarItemLocation::Hidden,
            "项目搜索视图自身不是编辑器实体，通用文档工具栏应隐藏"
        );
        TestView
    });
}

/// 搜索条在新活动 Item 上部署搜索；查询被派发给当前 Item。
#[gpui::test]
fn document_toolbar_deploys_search_on_the_active_item(cx: &mut TestAppContext) {
    let bar = document_toolbar(cx);
    cx.add_window_view(|window, cx| {
        let item = cx.new(|cx| TestItem {
            focus: cx.focus_handle(),
            path: None,
            exposes_search: true,
            last_query: None,
        });
        bar.update(cx, |bar, cx| {
            bar.set_active_pane_item(Some(&item as &dyn ItemHandle), window, cx);
            bar.deploy(Some("needle".into()), window, cx);
        });
        assert_eq!(item.read(cx).last_query.as_deref(), Some("needle"));
        TestView
    });
}
