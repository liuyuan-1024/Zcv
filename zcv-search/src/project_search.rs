//! 项目级跨文件内容搜索视图。
//!
//! 与文件内搜索复用 SearchBar UI、快捷键和查询协议，但持有独立状态机；
//! 本 Item 搜索整个 Project，并把 ordered excerpts 写入 MultiBuffer。

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString, Subscription,
    Task, WeakEntity, Window, div, prelude::*,
};
use zcv_actions::Deploy;
use zcv_editor::{Editor, EditorEvent};
use zcv_multi_buffer::{ExcerptLocation, MultiBuffer, MultiBufferExcerpt};
use zcv_project::Project;
use zcv_text::SearchQuery;
use zcv_theme::color;
use zcv_ui::Button;
use zcv_workspace::{
    Direction, Item, ItemEvent, ItemHandle, SearchEvent, SearchableItem, SearchableItemHandle,
    SerializedItemProvider, StatusItemView, ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView,
    Workspace,
};

use crate::search_bar::{SearchBar, SearchBarState};

const PROJECT_SEARCH_SERIALIZED_KIND: &str = "project-search";

#[derive(Clone, Debug)]
pub(crate) enum ProjectSearchEvent {
    Updated,
    Edited,
    DirtyChanged,
    OpenExcerptsRequested(Vec<ExcerptLocation>),
}

/// 输入防抖窗口：快速连续击键合并为一次全项目扫描。
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(100);
/// 流式装配的批大小：收满该数量片段才追加一次 MultiBuffer。
const SEARCH_BATCH_SIZE: usize = 16;
/// 批次间的让出间隔：每批装配后给主循环一次重绘机会。
const SEARCH_BATCH_YIELD: Duration = Duration::from_millis(1);

pub(crate) struct ProjectSearchView {
    project: Entity<Project>,
    results_editor: Entity<Editor>,
    excerpts: Entity<MultiBuffer>,
    // 最近一次成功搜索的命中数；None 表示尚未完成任何搜索。
    match_count: Option<usize>,
    search_generation: u64,
    debounce_task: Option<Task<()>>,
    pending_search: Option<Task<()>>,
    search_state: SearchBarState,
    _subscriptions: Vec<Subscription>,
}

pub(crate) struct ProjectSearchSerializedItemProvider {
    pub(crate) search_bar: Entity<ProjectSearchBar>,
}

impl SerializedItemProvider for ProjectSearchSerializedItemProvider {
    fn kind(&self) -> &'static str {
        PROJECT_SEARCH_SERIALIZED_KIND
    }

    fn restore(
        &self,
        state: serde_json::Value,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        let state: SearchBarState = match serde_json::from_value(state) {
            Ok(state) => state,
            Err(error) => {
                return Task::ready(Err(anyhow::anyhow!("项目搜索标签状态无效：{error}")));
            }
        };
        let view = cx.new(|cx| ProjectSearchView::new(project, cx));
        view.update(cx, |view, _| view.search_state = state.clone());
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.restore_state(state, window, cx)
        });
        Task::ready(Ok(Box::new(view) as Box<dyn ItemHandle>))
    }
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
                    EditorEvent::DiffHunksExpandedChanged => {}
                },
            ),
        ];
        Self {
            project,
            results_editor,
            excerpts,
            match_count: None,
            search_generation: 0,
            debounce_task: None,
            pending_search: None,
            search_state: SearchBarState {
                query: String::new(),
                case_sensitive: false,
                whole_word: false,
                regex: false,
            },
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

        let results = self
            .project
            .update(cx, |project, cx| project.search(query.clone(), cx));
        let search_task = results.task;
        let results_rx = results.rx;
        self.match_count = None;
        self.excerpts.update(cx, |buffer, cx| buffer.clear(cx));
        self.results_editor.update(cx, |editor, cx| {
            SearchableItem::clear_search(editor, window, cx)
        });
        cx.emit(SearchEvent::MatchesInvalidated);
        cx.notify();

        let project = self.project.clone();
        let results_editor = self.results_editor.clone();
        self.pending_search = Some(cx.spawn_in(window, async move |this, cx| {
            let _search_task = search_task;
            let mut batched = Vec::<MultiBufferExcerpt>::new();
            let mut match_count = 0usize;
            loop {
                // 被更新的查询取代时放弃本次流式装配；
                // 放弃通道会让后台在下次发送时感知并提前结束扫描。
                if this
                    .update_in(cx, |this, _, _| this.search_generation != generation)
                    .unwrap_or(true)
                {
                    break;
                }
                let item = match results_rx.recv().await {
                    Ok(item) => item,
                    // 通道关闭：后台扫描结束，装配剩余批次。
                    Err(_) => break,
                };
                // Project 已释放或文档注册失败时跳过该文件。
                let Ok(source) = project.update(cx, |project, cx| {
                    if let Some(buffer) = item.loaded_buffer {
                        project.register_loaded_buffer(item.path, buffer, cx)
                    } else {
                        project.open_buffer(&item.path, cx)
                    }
                }) else {
                    continue;
                };
                for excerpt in item.excerpts {
                    match_count += excerpt.matches.len();
                    batched.push(MultiBufferExcerpt::new(
                        source.clone(),
                        excerpt.range,
                        excerpt.matches,
                    ));
                }
                if batched.len() < SEARCH_BATCH_SIZE {
                    continue;
                }
                let batch = std::mem::take(&mut batched);
                this.update_in(cx, |this, _window, cx| {
                    this.append_search_batch(
                        batch,
                        match_count,
                        &results_editor,
                        query.clone(),
                        cx,
                    );
                })
                .ok();
                // 让出主循环：每批装配后重绘，结果渐进可见。
                cx.background_executor().timer(SEARCH_BATCH_YIELD).await;
            }
            this.update_in(cx, |this, _window, cx| {
                if !batched.is_empty() {
                    this.append_search_batch(batched, match_count, &results_editor, query, cx);
                }
                this.pending_search = None;
                cx.emit(SearchEvent::MatchesInvalidated);
                cx.emit(ProjectSearchEvent::Updated);
                cx.notify();
            })
            .ok();
        }));
    }

    /// 将新增片段追加到组合文档，并更新匹配高亮与命中计数。
    fn append_search_batch(
        &mut self,
        excerpts: Vec<MultiBufferExcerpt>,
        match_count: usize,
        results_editor: &Entity<Editor>,
        query: SearchQuery,
        cx: &mut Context<Self>,
    ) {
        let match_ranges = self
            .excerpts
            .update(cx, |buffer, cx| buffer.append_excerpts(excerpts, cx));
        results_editor.update(cx, |editor, cx| {
            editor.append_search_ranges(query, match_ranges, cx)
        });
        self.match_count = Some(match_count);
        cx.emit(SearchEvent::MatchesInvalidated);
        cx.notify();
    }

    fn reset_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_search = None;
        self.match_count = None;
        self.excerpts.update(cx, |buffer, cx| buffer.clear(cx));
        self.results_editor.update(cx, |editor, cx| {
            SearchableItem::clear_search(editor, window, cx)
        });
        cx.emit(SearchEvent::MatchesInvalidated);
        cx.emit(ProjectSearchEvent::Updated);
        cx.notify();
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
        let has_results = self.match_count.is_some_and(|count| count > 0);
        let show_empty = self.match_count == Some(0);

        div()
            .key_context("ProjectSearchView")
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.editor_background)
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

    fn serialized_pane_item(&self, _cx: &App) -> Option<zcv_workspace::SerializedPaneItem> {
        Some(zcv_workspace::SerializedPaneItem::Custom {
            kind: PROJECT_SEARCH_SERIALIZED_KIND.into(),
            state: serde_json::to_value(self.search_state.clone()).ok()?,
        })
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
        self.search_state = SearchBarState {
            query: query.query.clone(),
            case_sensitive: query.case_sensitive,
            whole_word: query.whole_word,
            regex: query.regex,
        };
        // 防抖合并击键；等窗内出现更新的查询（或搜索被清空）时放弃本次搜索。
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        let query = query.clone();
        self.debounce_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            this.update_in(cx, |this, window, cx| {
                if this.search_generation == generation {
                    this.run_search(query, window, cx);
                }
            })
            .ok();
        }));
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

    fn restore_state(
        &mut self,
        state: SearchBarState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.restore_state(state, window, cx);
        });
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
    let search_bar = cx.new(|cx| SearchBar::new("ProjectSearchBar", cx));
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
pub(crate) struct ProjectSearchButton {
    workspace: WeakEntity<Workspace>,
    search_bar: Entity<ProjectSearchBar>,
}

impl ProjectSearchButton {
    pub(crate) fn new(
        workspace: WeakEntity<Workspace>,
        search_bar: Entity<ProjectSearchBar>,
    ) -> Self {
        Self {
            workspace,
            search_bar,
        }
    }
}

impl StatusItemView for ProjectSearchButton {
    fn set_active_pane_item(&mut self, _item: Option<&dyn ItemHandle>, _cx: &mut Context<Self>) {}
}

impl Render for ProjectSearchButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let workspace = self.workspace.clone();
        let search_bar = self.search_bar.clone();
        Button::icon("search-button", "icons/magnifying_glass.svg")
            .label("项目搜索")
            .shortcut(&Deploy, cx)
            .on_click(move |_, window, cx| {
                workspace
                    .update(cx, |workspace, cx| {
                        crate::deploy_project_search(workspace, &search_bar, window, cx);
                    })
                    .ok();
            })
            .into_any_element()
    }
}
