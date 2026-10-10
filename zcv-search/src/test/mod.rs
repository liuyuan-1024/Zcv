use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, Render, TestAppContext, Window, div,
    prelude::*,
};
use zcv_language::LanguageRegistry;
use zcv_path::AbsolutePathBuf;
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
    let root =
        AbsolutePathBuf::canonicalize(std::path::Path::new(".")).expect("测试项目目录应可规范化");
    let project = cx.new(|cx| Project::new(root, Arc::new(LanguageRegistry::new()), cx));
    let language_registry = cx.read_entity(&project, |project, _| project.language_registry());
    let breadcrumbs = cx.new(|_| Breadcrumbs::new(project));
    cx.new(|cx| DocumentToolbar::new(preview_button, breadcrumbs, language_registry, cx))
}

#[gpui::test]
fn buffer_search_does_not_require_an_active_path(cx: &mut TestAppContext) {
    let bar = document_toolbar(cx);
    cx.add_window_view(|window, cx| {
        let editor = cx.new(|cx| Editor::single_line(Arc::new(LanguageRegistry::new()), cx));
        editor.update(cx, |editor, cx| editor.set_text("needle", cx));
        let location = bar.update(cx, |bar, cx| {
            let location = bar.set_active_pane_item(Some(&editor as &dyn ItemHandle), window, cx);
            bar.deploy(Some("needle".into()), window, cx);
            location
        });
        assert_eq!(location, ToolbarItemLocation::Secondary);
        assert_eq!(editor.read(cx).active_path(cx), None);
        assert_eq!(editor.read(cx).search_count(cx), (1, Some(0)));
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
        let location = bar.update(cx, |bar, cx| {
            bar.set_active_pane_item(Some(&item as &dyn ItemHandle), window, cx)
        });
        assert_eq!(location, ToolbarItemLocation::Hidden);
        bar.update(cx, |bar, cx| bar.deploy(Some("needle".into()), window, cx));
        assert_eq!(item.read(cx).last_query, None);
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

#[gpui::test]
fn hidden_document_search_does_not_search_a_composite_editor(cx: &mut TestAppContext) {
    let bar = document_toolbar(cx);
    cx.add_window_view(|window, cx| {
        let registry = Arc::new(LanguageRegistry::new());
        let document = cx.new({
            let registry = Arc::clone(&registry);
            move |cx| Editor::single_line(registry, cx)
        });
        document.update(cx, |editor, cx| editor.set_text("alpha", cx));
        bar.update(cx, |bar, cx| {
            bar.set_active_pane_item(Some(&document as &dyn ItemHandle), window, cx);
            bar.deploy(Some("alpha".into()), window, cx);
        });

        let inner_editor = cx.new(move |cx| Editor::single_line(registry, cx));
        inner_editor.update(cx, |editor, cx| editor.set_text("alpha", cx));
        let composite = cx.new(|cx| CompositeItem {
            focus: cx.focus_handle(),
            inner_editor: inner_editor.clone(),
        });
        let location = bar.update(cx, |bar, cx| {
            bar.set_active_pane_item(Some(&composite as &dyn ItemHandle), window, cx)
        });
        assert_eq!(location, ToolbarItemLocation::Hidden);
        assert_eq!(inner_editor.read(cx).search_count(cx), (0, None));
        let focus = inner_editor.read(cx).focus_handle();
        window.focus(&focus, cx);
        bar.update(cx, |bar, cx| bar.deploy(Some("alpha".into()), window, cx));
        assert!(focus.is_focused(window));
        TestView
    });
}

/// 项目搜索结果仍走 Editor 管线，但当前 Item 的搜索目标是项目搜索本身。
#[gpui::test]
fn project_search_view_acts_as_editor_and_owns_its_toolbar(cx: &mut TestAppContext) {
    let root =
        AbsolutePathBuf::canonicalize(std::path::Path::new(".")).expect("测试项目目录应可规范化");
    let project = cx.new(|cx| Project::new(root, Arc::new(LanguageRegistry::new()), cx));
    let view = cx.new(|cx| crate::project_search::ProjectSearchView::new(project, cx));
    let bar = document_toolbar(cx);
    cx.add_window_view(|window, cx| {
        let handle: &dyn ItemHandle = &view;
        assert!(
            handle.act_as::<Editor>(cx).is_some(),
            "搜索结果同样是编辑器，应通过 act_as_type 暴露结果编辑器"
        );
        assert_eq!(
            handle
                .as_searchable(cx)
                .expect("项目搜索应可搜索")
                .item_id(),
            view.entity_id(),
        );
        let location = bar.update(cx, |bar, cx| {
            bar.set_active_pane_item(Some(handle), window, cx)
        });
        assert_eq!(
            location,
            ToolbarItemLocation::Hidden,
            "项目搜索视图自身不是编辑器实体，通用文档工具栏应隐藏"
        );
        let focus = handle
            .act_as::<Editor>(cx)
            .expect("项目搜索应暴露结果编辑器")
            .read(cx)
            .focus_handle();
        window.focus(&focus, cx);
        bar.update(cx, |bar, cx| bar.deploy(Some("needle".into()), window, cx));
        assert!(focus.is_focused(window), "隐藏的文档搜索栏不得抢走焦点");
        TestView
    });
}

/// 搜索条在新活动 Item 上部署搜索；查询被派发给当前 Item。
#[gpui::test]
fn document_toolbar_deploys_search_on_the_active_item(cx: &mut TestAppContext) {
    let bar = document_toolbar(cx);
    cx.add_window_view(|window, cx| {
        let editor = cx.new(|cx| Editor::single_line(Arc::new(LanguageRegistry::new()), cx));
        editor.update(cx, |editor, cx| editor.set_text("needle", cx));
        bar.update(cx, |bar, cx| {
            bar.set_active_pane_item(Some(&editor as &dyn ItemHandle), window, cx);
            bar.deploy(Some("needle".into()), window, cx);
        });
        assert_eq!(editor.read(cx).search_count(cx), (1, Some(0)));
        TestView
    });
}
