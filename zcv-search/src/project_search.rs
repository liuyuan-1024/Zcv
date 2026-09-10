//! 项目级跨文件内容搜索视图。
//!
//! 与文件内搜索复用 SearchBar UI、快捷键和查询协议，但持有独立状态机；
//! 本 Item 搜索整个 Project，并把 ordered excerpts 写入 MultiBuffer。

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyContext, ParentElement, Render,
    SharedString, Styled, Subscription, Task, WeakEntity, Window, div, prelude::*,
};
use zcv_actions::{
    Backtab, ClearSearch, DeployProjectSearch, FindNext, FindPrevious, SelectAll, Tab,
    ToggleCaseSensitive, ToggleRegex, ToggleWholeWord,
};
use zcv_editor::{Editor, EditorEvent};
use zcv_multi_buffer::{ExcerptLocation, MultiBuffer, MultiBufferExcerpt};
use zcv_project::Project;
use zcv_text::SearchQuery;
use zcv_theme::{color, space};
use zcv_ui::{Button, MatchOption, MatchOptions, SearchInput};
use zcv_workspace::{
    Direction, Item, ItemEvent, ItemHandle, SearchEvent, SearchableItem, SearchableItemHandle,
    SerializedItemProvider, StatusItemView, Workspace,
};

const PROJECT_SEARCH_SERIALIZED_KIND: &str = "project-search";

/// 项目搜索标签自身持久化的查询状态。
///
/// 选项以 [`MatchOptions`] 表达并扁平序列化,旧布局中的 `case_sensitive` 等 JSON 键保持不变。
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct ProjectSearchState {
    query: String,
    #[serde(flatten)]
    options: MatchOptions,
}

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
    search_state: ProjectSearchState,
    search_bar_visible: bool,
    query_input: Option<Entity<Editor>>,
    input_subscriptions: Vec<Subscription>,
    toolbar: Entity<ProjectSearchToolbar>,
    _subscriptions: Vec<Subscription>,
}

/// 项目搜索的工具栏视图代理。
///
/// Pane 的工具栏条要求独立的 Render 实体(`Item::toolbar_view` 返回 `AnyView`),同一视图实体不能既作内容区又作工具栏条;
/// 本代理把工具栏区渲染委托给持有全部搜索会话状态的 [`ProjectSearchView`],自身零状态。
pub(crate) struct ProjectSearchToolbar {
    view: WeakEntity<ProjectSearchView>,
}

impl ProjectSearchToolbar {
    pub(crate) fn new(view: WeakEntity<ProjectSearchView>) -> Self {
        Self { view }
    }
}

impl Render for ProjectSearchToolbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.view.upgrade().map_or_else(
            || div().into_any_element(),
            |view| {
                view.read(cx)
                    .render_search_bar(window, cx, self.view.clone())
            },
        )
    }
}

pub(crate) struct ProjectSearchSerializedItemProvider;

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
        let state: ProjectSearchState = match serde_json::from_value(state) {
            Ok(state) => state,
            Err(error) => {
                return Task::ready(Err(anyhow::anyhow!("项目搜索标签状态无效：{error}")));
            }
        };
        let view = cx.new(|cx| ProjectSearchView::new(project, cx));
        view.update(cx, |view, cx| {
            view.search_state = state;
            view.deploy_search_bar(None, window, cx);
        });
        subscribe_to_open_excerpts(&view, window, cx);
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
                    EditorEvent::Error(_) => {}
                },
            ),
        ];
        let view = cx.weak_entity();
        let toolbar = cx.new(|_| ProjectSearchToolbar::new(view));
        Self {
            project,
            results_editor,
            excerpts,
            match_count: None,
            search_generation: 0,
            debounce_task: None,
            pending_search: None,
            search_state: ProjectSearchState {
                query: String::new(),
                options: MatchOptions::default(),
            },
            search_bar_visible: false,
            query_input: None,
            input_subscriptions: Vec::new(),
            toolbar,
            _subscriptions: subscriptions,
        }
    }

    fn ensure_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.query_input.is_some() {
            return;
        }

        let query_input = cx.new(|cx| Editor::auto_height(1, Some(4), cx));
        query_input.update(cx, |editor, cx| editor.set_placeholder_text("搜索...", cx));
        let weak = cx.weak_entity();
        self.input_subscriptions.push(window.subscribe(
            &query_input,
            cx,
            move |_, event: &EditorEvent, window, cx| {
                if *event != EditorEvent::Edited {
                    return;
                }
                if let Some(view) = weak.upgrade() {
                    view.update(cx, |view, cx| view.search_from_input(window, cx));
                }
            },
        ));
        self.query_input = Some(query_input);
    }

    fn deploy_search_bar(
        &mut self,
        query_seed: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_visible = self.search_bar_visible;
        self.search_bar_visible = true;
        self.ensure_search_input(window, cx);
        let seeded = query_seed.is_some();
        if let Some(seed) = query_seed {
            self.search_state.query = if self.search_state.options.regex {
                regex::escape(&seed)
            } else {
                seed
            };
        }
        let query_input = self.query_input.as_ref().expect("搜索输入框应已创建");
        query_input.update(cx, |editor, cx| {
            editor.set_text(&self.search_state.query, cx)
        });
        window.focus(&query_input.read(cx).focus_handle(), cx);
        window.dispatch_action(Box::new(SelectAll), cx);
        if !was_visible || seeded {
            self.search_from_input(window, cx);
        }
        cx.notify();
    }

    fn close_search_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_bar_visible = false;
        SearchableItem::clear_search(self, window, cx);
        window.focus(&self.focus_handle(cx), cx);
        cx.notify();
    }

    /// 把键盘焦点交给查询输入框。
    ///
    /// Item 的主焦点是结果编辑器，打开/激活项目搜索后需要显式让查询框可输入。
    fn focus_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.query_input.as_ref() {
            window.focus(&input.read(cx).focus_handle(), cx);
        }
    }

    fn search_from_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_state.query = self
            .query_input
            .as_ref()
            .map_or(String::new(), |input| input.read(cx).text(cx));
        let query = SearchQuery {
            query: self.search_state.query.clone(),
            case_sensitive: self.search_state.options.case_sensitive,
            whole_word: self.search_state.options.whole_word,
            regex: self.search_state.options.regex,
        };
        SearchableItem::search(self, &query, window, cx);
        cx.notify();
    }

    fn toggle_search_option(
        &mut self,
        option: MatchOption,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_state.options = self.search_state.options.toggled(option);
        self.search_from_input(window, cx);
    }

    fn move_active_match(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        SearchableItem::activate_match_in_direction(self, direction, 1, window, cx);
    }

    fn cycle_search_focus(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query_input = self.query_input.as_ref().expect("搜索输入框应已创建");
        let handles = [query_input.read(cx).focus_handle(), self.focus_handle(cx)];
        let Some(current) = handles.iter().position(|focus| focus.is_focused(window)) else {
            return;
        };
        let next = match direction {
            Direction::Next => (current + 1) % handles.len(),
            Direction::Prev => (current + handles.len() - 1) % handles.len(),
        };
        window.focus(&handles[next], cx);
        cx.stop_propagation();
    }

    pub(crate) fn render_search_bar(
        &self,
        _window: &mut Window,
        cx: &App,
        weak: WeakEntity<Self>,
    ) -> gpui::AnyElement {
        if !self.search_bar_visible {
            return div().into_any_element();
        }

        let query_input = self.query_input.as_ref().expect("搜索输入框应已创建");
        let (match_count, active_match_index) = SearchableItem::search_count(self, cx);
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("ProjectSearchBar");
        let expansion = {
            let snapshot = self.excerpts.read(cx).snapshot(cx);
            let paths = snapshot
                .excerpts()
                .iter()
                .map(|excerpt| excerpt.path())
                .collect::<Vec<_>>();
            let expanded = paths
                .iter()
                .any(|path| !self.results_editor.read(cx).is_buffer_folded(path));
            let weak = weak.clone();
            Button::icon(
                "project-search-expansion",
                if expanded {
                    "icons/chevron_down_up.svg"
                } else {
                    "icons/chevron_up_down.svg"
                },
            )
            .label(if expanded {
                "折叠全部文件"
            } else {
                "展开全部文件"
            })
            .on_click(move |_, _, cx| {
                if let Some(view) = weak.upgrade() {
                    view.update(cx, |view, cx| view.set_all_files_folded(expanded, cx));
                }
            })
            .into_any_element()
        };

        div()
            .key_context(key_context)
            .on_action({
                let weak = weak.clone();
                move |_: &FindNext, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.move_active_match(Direction::Next, window, cx)
                        });
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &FindPrevious, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.move_active_match(Direction::Prev, window, cx)
                        });
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &ClearSearch, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| view.close_search_bar(window, cx));
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &ToggleCaseSensitive, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.toggle_search_option(MatchOption::CaseSensitive, window, cx)
                        });
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &ToggleWholeWord, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.toggle_search_option(MatchOption::WholeWord, window, cx)
                        });
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &ToggleRegex, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.toggle_search_option(MatchOption::Regex, window, cx)
                        });
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Tab, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.cycle_search_focus(Direction::Next, window, cx)
                        });
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Backtab, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.cycle_search_focus(Direction::Prev, window, cx)
                        });
                    }
                }
            })
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(space::S6)
                    .child(expansion)
                    .child(
                        div().flex_1().min_w_0().child(
                            SearchInput::new(
                                "project-search",
                                query_input.clone().into_any_element(),
                            )
                            .options(self.search_state.options)
                            .on_toggle({
                                let weak = weak.clone();
                                move |option, window, cx| {
                                    if let Some(view) = weak.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.toggle_search_option(option, window, cx)
                                        });
                                    }
                                }
                            })
                            .count(active_match_index, match_count)
                            .on_previous({
                                let weak = weak.clone();
                                move |window, cx| {
                                    if let Some(view) = weak.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.move_active_match(Direction::Prev, window, cx)
                                        });
                                    }
                                }
                            })
                            .on_next({
                                let weak = weak.clone();
                                move |window, cx| {
                                    if let Some(view) = weak.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.move_active_match(Direction::Next, window, cx)
                                        });
                                    }
                                }
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn set_all_files_folded(&mut self, folded: bool, cx: &mut Context<Self>) {
        let mut paths = Vec::new();
        for excerpt in self.excerpts.read(cx).snapshot(cx).excerpts() {
            if !paths.iter().any(|path| path == excerpt.path()) {
                paths.push(excerpt.path().to_path_buf());
            }
        }
        self.results_editor.update(cx, |editor, cx| {
            for path in paths {
                if editor.is_buffer_folded(&path) != folded {
                    editor.toggle_buffer_fold(path, cx);
                }
            }
        });
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

    fn toolbar_view(&self, _self_handle: &Entity<Self>, _cx: &App) -> Option<gpui::AnyView> {
        Some(self.toolbar.clone().into())
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
        self.search_state = ProjectSearchState {
            query: query.query.clone(),
            options: MatchOptions {
                case_sensitive: query.case_sensitive,
                whole_word: query.whole_word,
                regex: query.regex,
            },
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

/// 把项目搜索的「打开片段」请求接到工作区打开文件。
///
/// 新建（deploy）与布局恢复（restore）两条创建路径都必须接线：
/// 订阅属于 Workspace，视图自身只发事件，恢复出的标签漏接就再没有人处理打开请求。
fn subscribe_to_open_excerpts(
    view: &Entity<ProjectSearchView>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    cx.subscribe_in(view, window, |workspace, _, event, window, cx| {
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
}

pub(crate) fn deploy(
    workspace: &mut Workspace,
    seed: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let pane = workspace.pane().clone();
    if let Some(existing) = pane
        .read(cx)
        .tabs()
        .iter()
        .find_map(|item| item.act_as::<ProjectSearchView>(cx))
    {
        let item_id = existing.entity_id();
        pane.update(cx, |pane, cx| pane.activate_tab(item_id, window, cx));
        // 已有标签重新打开搜索栏并聚焦查询框；activate_tab 自身不改变焦点。
        existing.update(cx, |view, cx| view.deploy_search_bar(None, window, cx));
        return;
    }

    let project = workspace.project().clone();
    let view = cx.new(|cx| ProjectSearchView::new(project, cx));
    view.update(cx, |view, cx| {
        view.deploy_search_bar(seed, window, cx);
    });
    subscribe_to_open_excerpts(&view, window, cx);
    let view_handle = view.clone();
    workspace.open_item(Box::new(view), window, cx);
    // open_item 会把焦点交给 Item 主内容（结果编辑器）；项目搜索打开后应直接可输入。
    view_handle.update(cx, |view, cx| view.focus_search_input(window, cx));
}

/// 状态栏中的项目搜索入口。
pub(crate) struct ProjectSearchButton {
    workspace: WeakEntity<Workspace>,
}

impl ProjectSearchButton {
    pub(crate) fn new(workspace: WeakEntity<Workspace>) -> Self {
        Self { workspace }
    }
}

impl StatusItemView for ProjectSearchButton {
    fn set_active_pane_item(&mut self, _item: Option<&dyn ItemHandle>, _cx: &mut Context<Self>) {}
}

impl Render for ProjectSearchButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let workspace = self.workspace.clone();
        Button::icon("search-button", "icons/magnifying_glass.svg")
            .label("项目搜索")
            .shortcut(&DeployProjectSearch, cx)
            .on_click(move |_, window, cx| {
                workspace
                    .update(cx, |workspace, cx| {
                        crate::deploy_project_search(workspace, window, cx);
                    })
                    .ok();
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};
    use zcv_text::{ByteOffset, TextRange};

    use super::*;

    /// 回归：布局恢复出的项目搜索标签必须与 deploy 新建的一样接上工作区的打开订阅。
    ///
    /// 漏接时点击「打开文件」与 alt-enter 都只发出事件而无人处理，表现为搜索结果无法打开文件。
    #[gpui::test]
    async fn restored_project_search_tab_opens_excerpt_files(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let root = directory.path().canonicalize().expect("项目根应可规范化");
        let file = root.join("needle.txt");
        std::fs::write(&file, "needle").expect("应创建测试文件");
        // 打开文件经 ItemProvider 注册表分发，测试同样需要文本 Provider。
        cx.update(zcv_editor::init);

        let provider = ProjectSearchSerializedItemProvider;
        let (workspace, cx) = cx.add_window_view({
            let root = root.clone();
            move |window, cx| Workspace::new(root, window, cx)
        });

        // 按布局恢复路径重建项目搜索标签，再像 restore_pane 一样放进 Pane。
        let state = serde_json::to_value(ProjectSearchState {
            query: "needle".into(),
            options: MatchOptions::default(),
        })
        .expect("搜索栏状态应可序列化");
        let restored = workspace.update_in(cx, |workspace, window, cx| {
            provider.restore(state, workspace.project().clone(), window, cx)
        });
        let item = restored.await.expect("项目搜索标签应可恢复");
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_item(item, window, cx)
        });

        let view = cx.read_entity(&workspace, |workspace, cx| {
            workspace
                .pane()
                .read(cx)
                .tabs()
                .iter()
                .find_map(|item| item.act_as::<ProjectSearchView>(cx))
                .expect("恢复出的标签应是项目搜索视图")
        });

        cx.read_entity(&view, |view, _| {
            assert!(view.search_bar_visible, "恢复的标签应重新打开搜索栏");
            assert_eq!(view.search_state.query, "needle", "查询状态应由视图恢复");
        });

        // 命中片段请求打开源文件：与点击「打开文件」和 alt-enter 发出的事件同一条路径。
        view.update(cx, |_, cx| {
            cx.emit(ProjectSearchEvent::OpenExcerptsRequested(vec![
                ExcerptLocation {
                    path: file.clone(),
                    source_range: TextRange::new(ByteOffset::ZERO, ByteOffset::ZERO)
                        .expect("同点源范围必须有效"),
                },
            ]));
        });
        cx.run_until_parked();

        cx.read_entity(&workspace, |workspace, cx| {
            let opened: Vec<_> = workspace
                .pane()
                .read(cx)
                .tabs()
                .iter()
                .filter_map(|item| item.item_path(cx))
                .collect();
            assert!(
                opened.contains(&file),
                "恢复的项目搜索标签应能打开命中文件，实际标签：{opened:?}"
            );
        });
    }

    /// 打开项目搜索后应直接聚焦查询输入框；Item 主焦点默认落在结果编辑器上。
    #[gpui::test]
    async fn deploying_project_search_focuses_query_input(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let root = directory.path().canonicalize().expect("项目根应可规范化");
        cx.update(zcv_editor::init);

        let (workspace, cx) = cx.add_window_view({
            let root = root.clone();
            move |window, cx| Workspace::new(root, window, cx)
        });

        // 首次打开：新建项目搜索标签。
        workspace.update_in(cx, |workspace, window, cx| {
            deploy(workspace, None, window, cx);
        });
        cx.run_until_parked();
        assert_query_input_focused(&workspace, cx, "新建标签后");

        // 已有标签：先把焦点移回结果编辑器，再重新打开项目搜索。
        let results_focus = cx.read_entity(&workspace, |workspace, cx| {
            project_search_view(workspace, cx)
                .read(cx)
                .results_editor
                .read(cx)
                .focus_handle()
        });
        cx.update(|window, cx| window.focus(&results_focus, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            deploy(workspace, None, window, cx);
        });
        cx.run_until_parked();
        assert_query_input_focused(&workspace, cx, "已有标签重新打开后");
    }

    fn project_search_view(
        workspace: &Workspace,
        cx: &gpui::App,
    ) -> gpui::Entity<ProjectSearchView> {
        workspace
            .pane()
            .read(cx)
            .tabs()
            .iter()
            .find_map(|item| item.act_as::<ProjectSearchView>(cx))
            .expect("应存在项目搜索标签")
    }

    fn assert_query_input_focused(
        workspace: &gpui::Entity<Workspace>,
        cx: &mut VisualTestContext,
        context: &str,
    ) {
        let input_focus = cx.read_entity(workspace, |workspace, cx| {
            project_search_view(workspace, cx)
                .read(cx)
                .query_input
                .as_ref()
                .expect("搜索输入框应已创建")
                .read(cx)
                .focus_handle()
        });
        cx.update(|window, _| {
            assert!(input_focus.is_focused(window), "{context}应聚焦查询输入框");
        });
    }
}
