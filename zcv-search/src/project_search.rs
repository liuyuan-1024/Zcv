//! 项目级跨文件内容搜索视图。
//!
//! 与文件内搜索复用 SearchBar UI、快捷键和查询协议，但持有独立状态机；
//! 本 Item 搜索整个 Project，并把 ordered excerpts 写入 MultiBuffer。

use std::path::PathBuf;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString, Subscription,
    Task, Window, div, prelude::*,
};
use zcv_actions::Deploy;
use zcv_editor::{Editor, EditorEvent};
use zcv_multi_buffer::{ExcerptLocation, MultiBuffer};
use zcv_project::Project;
use zcv_text::SearchQuery;
use zcv_theme::{color, space, typography};
use zcv_ui::Glyph;
use zcv_workspace::{
    Direction, Item, ItemEvent, ItemHandle, SearchEvent, SearchableItem, SearchableItemHandle,
    StatusItemView, ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView, Workspace,
};

use crate::search_bar::SearchBar;

#[derive(Clone, Debug)]
pub(crate) enum ProjectSearchEvent {
    Updated,
    Edited,
    DirtyChanged,
    OpenExcerptsRequested(Vec<ExcerptLocation>),
}

#[derive(Clone, Debug)]
enum SearchState {
    Idle,
    Searching,
    Results { match_count: usize },
    Error(String),
}

pub(crate) struct ProjectSearchView {
    project: Entity<Project>,
    results_editor: Entity<Editor>,
    excerpts: Entity<MultiBuffer>,
    state: SearchState,
    search_generation: u64,
    pending_search: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ProjectSearchView {
    pub(crate) fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let excerpts = cx.new(MultiBuffer::empty);
        let results_editor = cx.new(|cx| Editor::for_multi_buffer(excerpts.clone(), cx));
        let subscriptions = vec![
            cx.observe(&results_editor, |_, _, cx| cx.notify()),
            cx.subscribe(
                &results_editor,
                |_, _, event: &EditorEvent, cx| match event {
                    EditorEvent::Edited => cx.emit(ProjectSearchEvent::Edited),
                    EditorEvent::DirtyChanged => cx.emit(ProjectSearchEvent::DirtyChanged),
                    EditorEvent::OpenExcerptsRequested { locations, .. } => {
                        cx.emit(ProjectSearchEvent::OpenExcerptsRequested(locations.clone()));
                    }
                    EditorEvent::PathChanged => {}
                },
            ),
        ];
        Self {
            project,
            results_editor,
            excerpts,
            state: SearchState::Idle,
            search_generation: 0,
            pending_search: None,
            _subscriptions: subscriptions,
        }
    }

    fn run_search(&mut self, query: SearchQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        if query.query.is_empty() {
            self.reset_results(window, cx);
            return;
        }

        let task = self
            .project
            .update(cx, |project, cx| project.search(query.clone(), cx));
        self.state = SearchState::Searching;
        self.excerpts.update(cx, |buffer, cx| buffer.clear(cx));
        self.results_editor.update(cx, |editor, cx| {
            SearchableItem::clear_search(editor, window, cx)
        });
        cx.emit(SearchEvent::MatchesInvalidated);
        cx.notify();

        let excerpts = self.excerpts.clone();
        let results_editor = self.results_editor.clone();
        self.pending_search = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, _window, cx| {
                if this.search_generation != generation {
                    return;
                }
                this.pending_search = None;
                match result {
                    Ok(results) => {
                        let match_count = results.match_count;
                        excerpts.update(cx, |buffer, cx| {
                            buffer.set_excerpts(results.into_excerpts(), cx)
                        });
                        let match_ranges = excerpts.read(cx).match_ranges().to_vec();
                        results_editor.update(cx, |editor, cx| {
                            editor.set_search_ranges(query, match_ranges, cx)
                        });
                        this.state = SearchState::Results { match_count };
                    }
                    Err(error) => this.state = SearchState::Error(error.to_string()),
                }
                cx.emit(SearchEvent::MatchesInvalidated);
                cx.emit(ProjectSearchEvent::Updated);
                cx.notify();
            })
            .ok();
        }));
    }

    fn reset_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_search = None;
        self.state = SearchState::Idle;
        self.excerpts.update(cx, |buffer, cx| buffer.clear(cx));
        self.results_editor.update(cx, |editor, cx| {
            SearchableItem::clear_search(editor, window, cx)
        });
        cx.emit(SearchEvent::MatchesInvalidated);
        cx.emit(ProjectSearchEvent::Updated);
        cx.notify();
    }

    fn status_text(&self) -> Option<String> {
        match &self.state {
            SearchState::Idle => Some("在搜索栏输入内容以搜索整个项目".to_string()),
            SearchState::Searching => Some("正在搜索项目…".to_string()),
            SearchState::Results { .. } => None,
            SearchState::Error(error) => Some(format!("搜索失败：{error}")),
        }
    }
}

impl EventEmitter<ProjectSearchEvent> for ProjectSearchView {}
impl EventEmitter<SearchEvent> for ProjectSearchView {}

impl Focusable for ProjectSearchView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.results_editor.read(cx).focus_handle()
    }
}

impl Render for ProjectSearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let colors = color::current(cx);
        let has_results = matches!(
            self.state,
            SearchState::Results {
                match_count: 1..,
                ..
            }
        );
        let show_empty = matches!(self.state, SearchState::Results { match_count: 0, .. });
        let error = matches!(self.state, SearchState::Error(_));
        let status_text = self.status_text();

        div()
            .key_context("ProjectSearchView")
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.editor_background)
            .when_some(status_text, |element, status_text| {
                element.child(
                    div()
                        .flex_none()
                        .px(space::S8)
                        .py(space::S2)
                        .text_size(typography::ui() * 0.85)
                        .text_color(if error {
                            colors.status_error
                        } else {
                            colors.text_muted
                        })
                        .child(status_text),
                )
            })
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .when(has_results, |element| {
                        element.child(self.results_editor.clone())
                    })
                    .when(show_empty, |element| {
                        element
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(colors.text_muted)
                            .child("没有搜索结果")
                    }),
            )
    }
}

impl Item for ProjectSearchView {
    type Event = ProjectSearchEvent;

    fn tab_content_text(&self, _cx: &App) -> SharedString {
        "项目搜索".into()
    }

    fn tab_icon(&self, _cx: &App) -> Option<SharedString> {
        Some("icons/magnifying_glass.svg".into())
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        match event {
            ProjectSearchEvent::Updated => emit(ItemEvent::UpdateTab),
            ProjectSearchEvent::Edited => emit(ItemEvent::Edit),
            ProjectSearchEvent::DirtyChanged => emit(ItemEvent::UpdateTab),
            ProjectSearchEvent::OpenExcerptsRequested(_) => {}
        }
    }

    fn active_path(&self, cx: &App) -> Option<PathBuf> {
        self.results_editor
            .read(cx)
            .excerpt_location(cx)
            .map(|location| location.path)
    }

    fn multi_buffer(&self, _cx: &App) -> Option<Entity<MultiBuffer>> {
        Some(self.excerpts.clone())
    }

    fn can_save(&self, cx: &App) -> bool {
        <Editor as Item>::can_save(self.results_editor.read(cx), cx)
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.results_editor.read(cx).is_dirty(cx)
    }

    fn save(
        &mut self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        self.results_editor.update(cx, |editor, cx| {
            <Editor as Item>::save(editor, project, window, cx)
        })
    }

    fn breadcrumb_location(&self, _cx: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::Hidden
    }

    fn as_searchable(
        &self,
        self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn SearchableItemHandle>> {
        Some(Box::new(self_handle.clone()))
    }
}

impl SearchableItem for ProjectSearchView {
    fn supports_replace(&self) -> bool {
        false
    }

    fn search(&mut self, query: &SearchQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.run_search(query.clone(), window, cx);
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.reset_results(window, cx);
    }

    fn search_count(&self, cx: &App) -> (usize, Option<usize>) {
        SearchableItem::search_count(self.results_editor.read(cx), cx)
    }

    fn activate_match_in_direction(
        &mut self,
        direction: Direction,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.results_editor.update(cx, |editor, cx| {
            SearchableItem::activate_match_in_direction(editor, direction, count, window, cx)
        });
        cx.emit(SearchEvent::ActiveMatchChanged);
        cx.notify();
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

/// 项目搜索自己的搜索栏状态机；与 BufferSearchBar 只共享内部 SearchBar 实现。
pub(super) struct ProjectSearchBar {
    search_bar: Entity<SearchBar>,
}

impl ProjectSearchBar {
    fn new(search_bar: Entity<SearchBar>, cx: &mut Context<Self>) -> Self {
        cx.subscribe(&search_bar, |_, _, event: &ToolbarItemEvent, cx| {
            cx.emit(*event)
        })
        .detach();
        Self { search_bar }
    }

    pub(super) fn deploy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_bar
            .update(cx, |search_bar, cx| search_bar.deploy(window, cx));
    }
}

impl EventEmitter<ToolbarItemEvent> for ProjectSearchBar {}

impl ToolbarItemView for ProjectSearchBar {
    fn set_active_pane_item(
        &mut self,
        active_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        let Some(active_item) = active_item.filter(|item| is_project_search_item(*item, cx)) else {
            return ToolbarItemLocation::Hidden;
        };
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.set_active_item(Some(active_item), window, cx);
            search_bar.location()
        })
    }
}

impl Render for ProjectSearchBar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(self.search_bar.clone())
    }
}

pub(super) fn is_project_search_item(item: &dyn ItemHandle, cx: &App) -> bool {
    item.act_as::<ProjectSearchView>(cx).is_some()
}

pub(super) fn install_search_bar(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<ProjectSearchBar> {
    let search_bar = cx.new(SearchBar::new);
    let project_search_bar = cx.new(|cx| ProjectSearchBar::new(search_bar.clone(), cx));
    let toolbar = workspace.pane().read(cx).toolbar().clone();
    toolbar.update(cx, |toolbar, cx| {
        toolbar.add_item(project_search_bar.clone(), window, cx);
    });
    project_search_bar
}

pub(crate) fn deploy(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let pane = workspace.pane().clone();
    if let Some(existing) = pane
        .read(cx)
        .tabs()
        .iter()
        .find_map(|item| item.act_as::<ProjectSearchView>(cx))
    {
        let item_id = existing.entity_id();
        pane.update(cx, |pane, cx| pane.activate_tab(item_id, window, cx));
        return;
    }

    let project = workspace.project().clone();
    let view = cx.new(|cx| ProjectSearchView::new(project, cx));
    cx.subscribe_in(&view, window, |workspace, _, event, window, cx| {
        let ProjectSearchEvent::OpenExcerptsRequested(locations) = event else {
            return;
        };
        for location in locations {
            workspace.open_path_at(
                location.path.clone(),
                location.source_range.start().get()..location.source_range.end().get(),
                window,
                cx,
            );
        }
    })
    .detach();
    workspace.open_item(Box::new(view), window, cx);
}

/// 状态栏中的项目搜索入口。
pub struct ProjectSearchButton;

impl ProjectSearchButton {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ProjectSearchButton {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusItemView for ProjectSearchButton {
    fn set_active_pane_item(&mut self, _item: Option<&dyn ItemHandle>, _cx: &mut Context<Self>) {}
}

impl Render for ProjectSearchButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        Glyph::icon("search-button", "icons/magnifying_glass.svg")
            .label("项目搜索")
            .shortcut(&Deploy, cx)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(Deploy), cx))
            .into_any_element()
    }
}
