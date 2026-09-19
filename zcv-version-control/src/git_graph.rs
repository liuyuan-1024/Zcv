//! GitGraphView —— 只读图形化提交历史视图（workspace Item）。
//!
//! 数据来自 GitStore 的后台分批加载（`load_commit_graph`），lane 布局由 `crate::graph::GraphLayoutState` 逐行计算；
//! 视图侧只负责用 `gpui::canvas` 把每行的绘制指令画成圆点与连线，并渲染提交文本。
//! 与 `ProjectDiffView` 一致，通过 `deploy_at` 在 pane 中打开/复用，不做序列化持久化。

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    App, Bounds, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, Font,
    InteractiveElement, IntoElement, IsZero, MouseButton, PathBuilder, Pixels, Render, Rgba,
    ScrollHandle, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, Subscription,
    TextRun, UniformListScrollHandle, WeakEntity, Window, canvas, div, point, prelude::*, px,
    uniform_list,
};
use regex::RegexBuilder;
use zcv_actions::{
    Backtab, FindNext, FindPrevious, Tab, ToggleCaseSensitive, ToggleRegex, ToggleWholeWord,
};
use zcv_git::GraphCommit;

use crate::graph::{GraphLayoutState, GraphLine, GraphRowLayout};
use zcv_project::SearchQuery;
use zcv_project::{GitStoreEvent, Project};
use zcv_search::{SearchBar, SearchBarConfig, SearchBarSlots};
use zcv_theme::color::{self, ThemeColors};
use zcv_theme::{space, typography};
use zcv_ui::{ButtonLike, Scrollbar, TooltipSpec};
use zcv_workspace::{
    Direction, Item, ItemHandle, SearchEvent, SearchableItem, SerializedItemProvider,
    SerializedPaneItem, ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView,
    WeakSearchableItemHandle, Workspace, typography_for_window,
};

// ── 布局常量 ────────────────────────────────

/// 单条 lane 的水平宽度。
const LANE_WIDTH: Pixels = px(16.0);
/// 提交圆点半径。
const CIRCLE_RADIUS: Pixels = px(3.5);
/// 连线线宽。
const LINE_WIDTH: Pixels = px(1.5);
const COLUMN_COUNT: usize = 5;
const COLUMN_RESIZE_HANDLE_WIDTH: Pixels = space::S6;
const COLUMN_MIN_WIDTH: Pixels = space::S16;
const COLUMN_RESIZE_MAX_WIDTHS: [Pixels; COLUMN_COUNT] =
    [px(320.0), px(800.0), px(256.0), px(224.0), px(160.0)];
const COLUMN_CONTENT_MAX_WIDTHS: [Pixels; COLUMN_COUNT] =
    [px(256.0), px(640.0), px(192.0), px(160.0), px(112.0)];
/// 单批加载的提交数上限。
const BATCH_SIZE: usize = 100;
/// 距列表末尾多少行时预加载下一批。
const PRELOAD_ROWS: usize = 10;
const GIT_GRAPH_SERIALIZED_KIND: &str = "git-graph";

/// 一行 = 一条提交数据 + 其逐行布局指令。
#[derive(Clone)]
struct GraphRow {
    commit: GraphCommit,
    layout: GraphRowLayout,
}

/// 列边界的拖拽载荷；
/// 实际宽度由 GitGraphView 持有，避免建立第二份列状态。
struct DraggedGitGraphColumn(usize);

impl Render for DraggedGitGraphColumn {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

struct ColumnResizeState {
    boundary: usize,
    last_x: Pixels,
}

#[derive(Clone)]
struct GitGraphRowRenderContext {
    colors: ThemeColors,
    palette: [Rgba; 6],
    row_height: Pixels,
    column_widths: [Pixels; COLUMN_COUNT],
    graph_content_width: Pixels,
    weak: WeakEntity<GitGraphView>,
}

pub(crate) struct GitGraphView {
    focus: FocusHandle,
    project: Entity<Project>,
    /// 已加载并完成布局的行（按 `git log` 顺序）。
    rows: Vec<GraphRow>,
    /// 跨批次承载 lane 占用状态，分批追加时复用同一实例。
    layout: GraphLayoutState,
    /// 下一批加载的偏移量：已加载的提交数量（`None` 表示从历史开头开始）。
    offset: Option<usize>,
    /// 使分支或仓库切换时已经在途的旧加载结果失效。
    load_generation: u64,
    loading: bool,
    /// 无更多提交（加载到空批或不足一批）。
    reached_end: bool,
    /// 当前选中行下标。
    selected: Option<usize>,
    search_matches: Vec<usize>,
    active_search_match: Option<usize>,
    scroll_handle: UniformListScrollHandle,
    scrollbar: Scrollbar<UniformListScrollHandle>,
    horizontal_scroll_handle: ScrollHandle,
    horizontal_scrollbar: Scrollbar<ScrollHandle>,
    column_widths: [Pixels; COLUMN_COUNT],
    content_fit_widths: [Pixels; COLUMN_COUNT],
    content_fit_row_count: usize,
    content_fit_font_size: Pixels,
    column_widths_customized: [bool; COLUMN_COUNT],
    pending_column_reset: Option<usize>,
    column_resize: Option<ColumnResizeState>,
    _git_subscription: Subscription,
    /// 共享搜索栏会话：查询、匹配选项、可见性与按键接线由它唯一持有。
    search_bar: Entity<SearchBar>,
}

/// Git 提交图的搜索工具栏。
///
/// 作为 Pane 工具项存在：活动 Item 是提交图视图时显示搜索栏，否则隐藏；
/// 搜索目标是提交图视图自身（它实现 SearchableItem，命中计算保留在视图领域逻辑中）。
pub(crate) struct GitGraphToolbar {
    active_view: Option<Entity<GitGraphView>>,
    search_bar: Option<Entity<SearchBar>>,
}

impl GitGraphToolbar {
    pub(crate) fn new() -> Self {
        Self {
            active_view: None,
            search_bar: None,
        }
    }
}

impl EventEmitter<ToolbarItemEvent> for GitGraphToolbar {}

impl ToolbarItemView for GitGraphToolbar {
    fn set_active_pane_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        self.active_view = item.and_then(|item| item.act_as::<GitGraphView>(cx));
        let Some(view) = self.active_view.clone() else {
            if let Some(bar) = self.search_bar.take() {
                bar.update(cx, |bar, cx| bar.set_target(None, window, cx));
            }
            return ToolbarItemLocation::Hidden;
        };
        let bar = view.read(cx).search_bar.clone();
        // 同一视图重复激活时保留搜索会话；切换到另一视图时解除旧栏目标绑定。
        if let Some(previous) = self.search_bar.replace(bar.clone())
            && previous.entity_id() != bar.entity_id()
        {
            previous.update(cx, |bar, cx| bar.set_target(None, window, cx));
        }
        // 搜索目标是提交图视图自身；以弱句柄保存，避免与视图持有的搜索栏构成强引用环。
        let target: Box<dyn WeakSearchableItemHandle> = Box::new(view.downgrade());
        bar.update(cx, |bar, cx| bar.set_target(Some(target), window, cx));
        ToolbarItemLocation::PrimaryLeft
    }
}

impl Render for GitGraphToolbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(search_bar) = self.search_bar.clone() else {
            return div().into_any_element();
        };
        search_bar
            .update(cx, |bar, cx| {
                bar.render(SearchBarSlots::default(), window, cx)
            })
            .into_any_element()
    }
}

impl GitGraphView {
    fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let scroll_handle = UniformListScrollHandle::default();
        let scrollbar = Scrollbar::vertical(scroll_handle.clone());
        let git_store = project.read(cx).git_store();
        let search_bar = cx.new(|cx| {
            SearchBar::new(
                SearchBarConfig {
                    id_prefix: "git-graph",
                    key_context: "GitGraphSearchBar",
                    supports_replace: false,
                    query_placeholder: "搜索…",
                    replace_placeholder: "替换为…",
                    dismissible: false,
                },
                cx,
            )
        });
        let git_subscription = cx.subscribe(&git_store, |view, _, event, cx| {
            if matches!(
                event,
                GitStoreEvent::Repositories
                    | GitStoreEvent::Head
                    | GitStoreEvent::ActiveRepositoryChanged
            ) {
                view.reload(cx);
            }
        });
        let horizontal_scroll_handle = ScrollHandle::new();
        let mut view = Self {
            focus,
            project,
            rows: Vec::new(),
            layout: GraphLayoutState::new(),
            offset: None,
            load_generation: 0,
            loading: false,
            reached_end: false,
            selected: None,
            search_matches: Vec::new(),
            active_search_match: None,
            scroll_handle,
            scrollbar,
            horizontal_scroll_handle: horizontal_scroll_handle.clone(),
            horizontal_scrollbar: Scrollbar::horizontal(horizontal_scroll_handle),
            column_widths: [COLUMN_MIN_WIDTH; COLUMN_COUNT],
            content_fit_widths: [COLUMN_MIN_WIDTH; COLUMN_COUNT],
            content_fit_row_count: 0,
            content_fit_font_size: px(0.0),
            column_widths_customized: [false; COLUMN_COUNT],
            pending_column_reset: None,
            column_resize: None,
            _git_subscription: git_subscription,
            search_bar,
        };
        if git_store.read(cx).is_repository_scan_ready() {
            view.load_more(cx);
        }
        view
    }

    fn begin_column_resize(&mut self, boundary: usize, x: Pixels, cx: &mut Context<Self>) {
        if boundary + 1 >= COLUMN_COUNT {
            return;
        }
        self.column_resize = Some(ColumnResizeState {
            boundary,
            last_x: x,
        });
        cx.notify();
    }

    fn resize_column(&mut self, boundary: usize, delta: Pixels, cx: &mut Context<Self>) {
        let allowed_delta = resize_column_widths(&mut self.column_widths, boundary, delta);
        if allowed_delta.is_zero() {
            return;
        }
        self.column_widths_customized[boundary] = true;
        self.column_widths_customized[boundary + 1] = true;
        cx.notify();
    }

    fn reset_column(&mut self, boundary: usize, cx: &mut Context<Self>) {
        if boundary >= COLUMN_COUNT - 1 {
            return;
        }
        self.column_widths_customized[boundary] = false;
        self.pending_column_reset = Some(boundary);
        cx.notify();
    }

    fn finish_column_resize(&mut self, cx: &mut Context<Self>) {
        if self.column_resize.take().is_some() {
            cx.notify();
        }
    }

    /// 清空旧仓库或旧分支的投影，并从全部 refs 重新加载。
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.load_generation = self.load_generation.wrapping_add(1);
        self.rows.clear();
        self.layout = GraphLayoutState::new();
        self.content_fit_row_count = 0;
        self.offset = None;
        self.loading = false;
        self.reached_end = false;
        self.selected = None;
        self.search_matches.clear();
        self.active_search_match = None;
        self.load_more(cx);
    }

    /// 加载下一批提交；`loading`/`reached_end` 时跳过。后台读完成后回到实体逐条布局追加。
    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading
            || self.reached_end
            || !self
                .project
                .read(cx)
                .git_store()
                .read(cx)
                .is_repository_scan_ready()
        {
            return;
        }
        self.loading = true;
        let git_store = self.project.read(cx).git_store();
        let offset = self.offset;
        let generation = self.load_generation;
        let load = git_store.read(cx).load_commit_graph(offset, BATCH_SIZE);
        cx.spawn(async move |this, cx| {
            let commits = load.await;
            this.update(cx, |view, cx| {
                if view.load_generation != generation {
                    return;
                }
                view.loading = false;
                match commits {
                    Ok(commits) => view.append_commits(commits, cx),
                    // 读取失败按“到底”处理，避免反复重试刷屏。
                    Err(_) => {
                        view.reached_end = true;
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// 把一批提交逐条布局后追加到 `rows`，并推进偏移量/到底标记。
    fn append_commits(&mut self, commits: Vec<GraphCommit>, cx: &mut Context<Self>) {
        if commits.is_empty() {
            self.reached_end = true;
            cx.notify();
            return;
        }
        let batch_len = commits.len();
        for commit in commits {
            let layout = self.layout.push(&commit);
            self.rows.push(GraphRow { commit, layout });
        }
        self.offset = Some(self.rows.len());
        if batch_len < BATCH_SIZE {
            self.reached_end = true;
        }
        let query = self.search_bar.read(cx).query_text(cx);
        if query.is_empty() {
            cx.notify();
        } else {
            let options = self.search_bar.read(cx).options();
            self.run_search(
                &SearchQuery {
                    query,
                    case_sensitive: options.case_sensitive,
                    whole_word: options.whole_word,
                    regex: options.regex,
                },
                cx,
            );
        }
    }
}

impl EventEmitter<SearchEvent> for GitGraphView {}

impl Focusable for GitGraphView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.search_bar.read(cx).query_focus_handle(cx)
    }
}

impl Render for GitGraphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *color::current(cx);
        let palette = lane_palette(&colors);
        let type_scale = typography_for_window(window, cx);
        let row_height = row_height(type_scale.content_line());
        // 视图体聚焦时仍走 GitGraphSearchBar 键位上下文：
        // 提交图自身没有按键处理，搜索 action 一律转发给它持有的 SearchBar。
        // 转发用弱句柄而非 cx.listener，后者会在回调期间租借视图，而 SearchBar 的导航/选项操作又要回写本视图，造成同一实体二次租借。
        let root = div()
            .debug_selector(|| "git-graph-view".into())
            .size_full()
            .track_focus(&self.focus)
            .key_context("GitGraphSearchBar")
            .on_action({
                let bar = self.search_bar.downgrade();
                move |_: &FindNext, window, cx| {
                    if let Some(bar) = bar.upgrade() {
                        bar.update(cx, |bar, cx| bar.find_next(window, cx));
                    }
                }
            })
            .on_action({
                let bar = self.search_bar.downgrade();
                move |_: &FindPrevious, window, cx| {
                    if let Some(bar) = bar.upgrade() {
                        bar.update(cx, |bar, cx| bar.find_previous(window, cx));
                    }
                }
            })
            .on_action({
                let bar = self.search_bar.downgrade();
                move |_: &ToggleCaseSensitive, window, cx| {
                    if let Some(bar) = bar.upgrade() {
                        bar.update(cx, |bar, cx| bar.toggle_case_sensitive(window, cx));
                    }
                }
            })
            .on_action({
                let bar = self.search_bar.downgrade();
                move |_: &ToggleWholeWord, window, cx| {
                    if let Some(bar) = bar.upgrade() {
                        bar.update(cx, |bar, cx| bar.toggle_whole_word(window, cx));
                    }
                }
            })
            .on_action({
                let bar = self.search_bar.downgrade();
                move |_: &ToggleRegex, window, cx| {
                    if let Some(bar) = bar.upgrade() {
                        bar.update(cx, |bar, cx| bar.toggle_regex(window, cx));
                    }
                }
            })
            .on_action({
                let bar = self.search_bar.downgrade();
                move |_: &Tab, window, cx| {
                    if let Some(bar) = bar.upgrade() {
                        bar.update(cx, |bar, cx| bar.focus_query(window, cx));
                    }
                }
            })
            .on_action({
                let bar = self.search_bar.downgrade();
                move |_: &Backtab, window, cx| {
                    if let Some(bar) = bar.upgrade() {
                        bar.update(cx, |bar, cx| bar.focus_query(window, cx));
                    }
                }
            })
            .bg(colors.editor_background)
            // 提交文本属于内容：字号走内容通道，字体族沿用 UI 比例字体，只有短 SHA 用等宽。
            .font(typography::ui_font())
            .text_size(type_scale.content_size())
            .text_color(colors.text);
        if self.rows.is_empty() {
            let message = if self.loading {
                "加载中…"
            } else {
                "暂无提交历史"
            };
            return root.child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .text_color(colors.text_placeholder)
                    .child(message),
            );
        }

        let has_query = !self.search_bar.read(cx).query_text(cx).is_empty();
        let len = if !has_query {
            self.rows.len()
        } else {
            self.search_matches.len()
        };
        self.update_content_fit_widths(window, cx);
        let weak = cx.weak_entity();
        let column_widths = self.column_widths;
        let graph_content_width = graph_column_width(&self.rows);
        let content_width = column_widths
            .iter()
            .copied()
            .fold(px(0.0), |total, width| total + width);
        let resize_weak = weak.clone();
        let mouse_up_weak = weak.clone();
        let root = root
            .on_drag_move::<DraggedGitGraphColumn>(move |event, window, cx| {
                let boundary = event.drag(cx).0;
                let x = event.event.position.x;
                if let Some(view) = resize_weak.upgrade() {
                    view.update(cx, |view, cx| {
                        let Some(resize) = view.column_resize.as_ref() else {
                            return;
                        };
                        if resize.boundary != boundary {
                            return;
                        }
                        let delta = x - resize.last_x;
                        view.resize_column(boundary, delta, cx);
                        if let Some(resize) = view.column_resize.as_mut() {
                            resize.last_x = x;
                        }
                    });
                    window.refresh();
                    cx.stop_propagation();
                }
            })
            .on_mouse_up(MouseButton::Left, move |_, _window, cx| {
                if let Some(view) = mouse_up_weak.upgrade() {
                    view.update(cx, |view, cx| view.finish_column_resize(cx));
                }
            });
        let list_weak = weak.clone();
        let row_context = GitGraphRowRenderContext {
            colors,
            palette,
            row_height,
            column_widths,
            graph_content_width,
            weak: list_weak.clone(),
        };
        let list = uniform_list("git-graph-list", len, move |range, _window, cx| {
            let Some(view) = list_weak.upgrade() else {
                return Vec::new();
            };
            // 仅克隆可见行，避免整表逐帧复制；
            // 克隆后释放借用，便于随后触发加载。
            let visible: Vec<(usize, GraphRow, bool)> = {
                let read = view.read(cx);
                range
                    .clone()
                    .filter_map(|display_index| {
                        let index = if !has_query {
                            display_index
                        } else {
                            *read.search_matches.get(display_index)?
                        };
                        read.rows
                            .get(index)
                            .map(|row| (index, row.clone(), read.selected == Some(index)))
                    })
                    .collect()
            };
            // 渲染到接近末尾时预加载下一批（load_more 内部对 loading/reached_end 幂等）。
            if range.end >= len.saturating_sub(PRELOAD_ROWS) {
                view.update(cx, |view, cx| view.load_more(cx));
            }
            visible
                .into_iter()
                .map(|(index, row, selected)| {
                    render_graph_row(&row, index, selected, row_context.clone()).into_any_element()
                })
                .collect()
        })
        .w(content_width)
        .flex_1()
        .min_h_0()
        .track_scroll(&self.scroll_handle);

        let content = div()
            .w(content_width)
            .flex_none()
            .h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(render_graph_header(
                column_widths,
                colors,
                &weak,
                row_height,
            ))
            .child(list);

        let horizontal_scroll = div()
            .id("git-graph-horizontal-scroll")
            .size_full()
            .overflow_x_scroll()
            .restrict_scroll_to_axis()
            .track_scroll(&self.horizontal_scroll_handle)
            .child(content);
        root.child(
            div()
                .relative()
                .size_full()
                .child(horizontal_scroll)
                .child(div().absolute().inset_0().child(self.scrollbar.clone()))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(self.horizontal_scrollbar.clone()),
                ),
        )
    }
}

impl GitGraphView {
    /// 根据已加载提交的实际字宽更新尚未手动调整的列。
    fn update_content_fit_widths(&mut self, window: &Window, cx: &App) {
        let font_size = typography_for_window(window, cx).content_size();
        if self.content_fit_row_count != self.rows.len() || self.content_fit_font_size != font_size
        {
            self.content_fit_widths = content_fit_column_widths(&self.rows, window, cx);
            self.content_fit_row_count = self.rows.len();
            self.content_fit_font_size = font_size;
        }

        let pending_reset = self.pending_column_reset.take();
        if let Some(boundary) = pending_reset {
            reset_column_width(
                &mut self.column_widths,
                boundary,
                self.content_fit_widths[boundary],
            );
        }

        for index in 0..COLUMN_COUNT {
            if !self.column_widths_customized[index] && Some(index) != pending_reset {
                self.column_widths[index] = self.content_fit_widths[index];
            }
        }
    }
}

impl Item for GitGraphView {
    type Event = SearchEvent;

    fn tab_content_text(&self, _cx: &App) -> SharedString {
        self.project
            .read(_cx)
            .root()
            .and_then(|root| root.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "版本控制图".to_owned())
            .into()
    }

    fn tab_icon(&self, _cx: &App) -> Option<SharedString> {
        Some("icons/git_graph.svg".into())
    }

    fn serialized_pane_item(&self, cx: &App) -> Option<SerializedPaneItem> {
        Some(SerializedPaneItem::Custom {
            kind: GIT_GRAPH_SERIALIZED_KIND.into(),
            state: serde_json::json!({
                "query": self.search_bar.read(cx).query_text(cx),
                "options": self.search_bar.read(cx).options(),
            }),
        })
    }
}

/// 从布局恢复 Git 提交图；提交数据始终根据当前项目仓库重新加载。
pub struct GitGraphSerializedItemProvider;

impl SerializedItemProvider for GitGraphSerializedItemProvider {
    fn kind(&self) -> &'static str {
        GIT_GRAPH_SERIALIZED_KIND
    }

    fn restore(
        &self,
        state: serde_json::Value,
        project: Entity<Project>,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> gpui::Task<anyhow::Result<Box<dyn ItemHandle>>> {
        let view = cx.new(|cx| GitGraphView::new(project, cx));
        let query = state
            .get("query")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let options = state
            .get("options")
            .and_then(serde_json::Value::as_object)
            .map(|object| {
                serde_json::from_value(serde_json::Value::Object(object.clone()))
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        view.update(cx, |view, cx| {
            let search_bar = view.search_bar.clone();
            search_bar.update(cx, |bar, cx| bar.restore(query, options, cx));
        });
        gpui::Task::ready(Ok(Box::new(view) as Box<dyn ItemHandle>))
    }
}

impl GitGraphView {
    fn scroll_to_active_match(&self) {
        if let Some(match_index) = self.active_search_match
            && let Some(row_index) = self.search_matches.get(match_index)
        {
            self.scroll_handle
                .scroll_to_item(*row_index, ScrollStrategy::Center);
        }
    }
}

impl SearchableItem for GitGraphView {
    fn supports_replace(&self) -> bool {
        false
    }

    fn search(&mut self, query: &SearchQuery, _window: &mut Window, cx: &mut Context<Self>) {
        self.run_search(query, cx);
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        GitGraphView::clear_search(self, window, cx);
    }

    fn search_count(&self, cx: &App) -> (usize, Option<usize>) {
        GitGraphView::search_count(self, cx)
    }

    fn activate_match_in_direction(
        &mut self,
        direction: Direction,
        count: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active_match(direction, count, cx);
    }

    fn replace_current(
        &mut self,
        replacement: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        GitGraphView::replace_current(self, replacement, window, cx)
    }

    fn replace_all(
        &mut self,
        replacement: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        GitGraphView::replace_all(self, replacement, window, cx)
    }
}

impl GitGraphView {
    fn run_search(&mut self, query: &SearchQuery, cx: &mut Context<Self>) {
        self.search_matches.clear();
        self.active_search_match = None;

        if query.query.is_empty() {
            cx.emit(SearchEvent::MatchesInvalidated);
            cx.notify();
            return;
        }

        let pattern = if query.regex {
            query.query.clone()
        } else if query.whole_word {
            format!(r"\b(?:{})\b", regex::escape(&query.query))
        } else {
            regex::escape(&query.query)
        };
        let Ok(regex) = RegexBuilder::new(&pattern)
            .case_insensitive(!query.case_sensitive)
            .build()
        else {
            cx.emit(SearchEvent::MatchesInvalidated);
            cx.notify();
            return;
        };

        for (index, row) in self.rows.iter().enumerate() {
            let commit = &row.commit;
            let haystack = format!(
                "{}\n{}\n{}\n{}",
                commit.subject,
                commit.author_name,
                commit.oid,
                commit.refs.join(" ")
            );
            if regex.is_match(&haystack) {
                self.search_matches.push(index);
            }
        }
        self.active_search_match = (!self.search_matches.is_empty()).then_some(0);
        self.selected = self
            .active_search_match
            .and_then(|match_index| self.search_matches.get(match_index).copied());
        self.scroll_to_active_match();
        cx.emit(SearchEvent::MatchesInvalidated);
        if self.active_search_match.is_some() {
            cx.emit(SearchEvent::ActiveMatchChanged);
        }
        cx.notify();
    }

    fn clear_search(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.search_matches.clear();
        self.active_search_match = None;
        cx.emit(SearchEvent::MatchesInvalidated);
        cx.notify();
    }

    fn search_count(&self, _cx: &App) -> (usize, Option<usize>) {
        (
            self.search_matches.len(),
            self.active_search_match.map(|index| index + 1),
        )
    }

    fn move_active_match(&mut self, direction: Direction, count: usize, cx: &mut Context<Self>) {
        if self.search_matches.is_empty() {
            return;
        }
        let current = self.active_search_match.unwrap_or(0);
        let len = self.search_matches.len();
        let offset = count % len;
        let next = match direction {
            Direction::Next => (current + offset) % len,
            Direction::Prev => (current + len - offset) % len,
        };
        self.active_search_match = Some(next);
        self.selected = self.search_matches.get(next).copied();
        self.scroll_to_active_match();
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

/// 打开或复用版本控制图 Item（参考 `project_diff::deploy_at`）。
pub fn deploy_at(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let pane = workspace.pane().clone();
    if let Some(existing) = pane
        .read(cx)
        .tabs()
        .iter()
        .find_map(|item| item.act_as::<GitGraphView>(cx))
    {
        let item_id = existing.entity_id();
        pane.update(cx, |pane, cx| pane.activate_tab(item_id, window, cx));
        window.focus(&existing.read(cx).focus_handle(cx), cx);
        return;
    }

    let project = workspace.project().clone();
    let view = cx.new(|cx| GitGraphView::new(project, cx));
    let focus = pane.update(cx, |pane, cx| {
        pane.open_item(Box::new(view), false, window, cx)
    });
    window.focus(&focus, cx);
}

// ═══ 渲染辅助 ═══════════════════════════════════════════════════════

/// 单行高度：由内容行高派生（+ 少量竖直留白），随内容字号缩放。
/// uniform_list 要求所有行等高，故集中在此计算。
fn row_height(line_height: Pixels) -> Pixels {
    line_height + space::S4
}

/// 分支配色板：取主题终端 ANSI 色，随主题切换（无独立 accent 序列，故复用这组区分度高的色）。
fn lane_palette(colors: &ThemeColors) -> [Rgba; 6] {
    [
        colors.terminal_ansi_blue,
        colors.terminal_ansi_green,
        colors.terminal_ansi_magenta,
        colors.terminal_ansi_cyan,
        colors.terminal_ansi_yellow,
        colors.terminal_ansi_red,
    ]
}

/// 在画布上绘制一行的连线与圆点（坐标以画布 bounds 为原点）。
fn paint_graph(
    bounds: Bounds<Pixels>,
    layout: &GraphRowLayout,
    palette: &[Rgba; 6],
    window: &mut Window,
) {
    let top = bounds.origin.y;
    let bottom = bounds.origin.y + bounds.size.height;
    let center_y = bounds.origin.y + bounds.size.height / 2.0;
    let lane_x = |lane: usize| bounds.origin.x + lane as f32 * LANE_WIDTH + LANE_WIDTH / 2.0;
    let dot_x = lane_x(layout.dot_lane);

    for line in &layout.lines {
        let (builder, color) = match line {
            // 竖直贯穿整行。
            GraphLine::Pass { lane, color } => {
                let x = lane_x(*lane);
                let mut builder = PathBuilder::stroke(LINE_WIDTH);
                builder.move_to(point(x, top));
                builder.line_to(point(x, bottom));
                (builder, *color)
            }
            // 上半行：从行顶汇入圆点（同 lane 为竖直，异 lane 圆角拐弯）。
            GraphLine::MergeIn { from_lane, color } => {
                let from_x = lane_x(*from_lane);
                let mut builder = PathBuilder::stroke(LINE_WIDTH);
                builder.move_to(point(from_x, top));
                if *from_lane == layout.dot_lane {
                    builder.line_to(point(dot_x, center_y));
                } else {
                    builder.curve_to(point(dot_x, center_y), point(from_x, center_y));
                }
                (builder, *color)
            }
            // 下半行：从圆点分叉到行底（同 lane 为竖直，异 lane 圆角拐弯）。
            GraphLine::ForkOut { to_lane, color } => {
                let to_x = lane_x(*to_lane);
                let mut builder = PathBuilder::stroke(LINE_WIDTH);
                builder.move_to(point(dot_x, center_y));
                if *to_lane == layout.dot_lane {
                    builder.line_to(point(to_x, bottom));
                } else {
                    builder.curve_to(point(to_x, bottom), point(to_x, center_y));
                }
                (builder, *color)
            }
        };
        if let Ok(path) = builder.build() {
            window.paint_path(path, palette[color % palette.len()]);
        }
    }

    draw_commit_circle(
        dot_x,
        center_y,
        palette[layout.dot_color % palette.len()],
        window,
    );
}

/// 用两段半圆弧填充一个提交圆点。
fn draw_commit_circle(center_x: Pixels, center_y: Pixels, color: Rgba, window: &mut Window) {
    let radius = CIRCLE_RADIUS;
    let mut builder = PathBuilder::fill();
    builder.move_to(point(center_x + radius, center_y));
    builder.arc_to(
        point(radius, radius),
        px(0.0),
        false,
        true,
        point(center_x - radius, center_y),
    );
    builder.arc_to(
        point(radius, radius),
        px(0.0),
        false,
        true,
        point(center_x + radius, center_y),
    );
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// 渲染单行：左侧图形画布 + 右侧文本区；点击选中高亮。
fn graph_column(width: Pixels, colors: &ThemeColors, with_right_border: bool) -> gpui::Div {
    let mut column = div().w(width).h_full().flex().items_center().flex_none();
    if with_right_border {
        column = column.border_r_1().border_color(colors.border_variant);
    }
    column
}

fn render_graph_row(
    row: &GraphRow,
    index: usize,
    selected: bool,
    context: GitGraphRowRenderContext,
) -> impl IntoElement {
    let GitGraphRowRenderContext {
        colors,
        palette,
        row_height,
        column_widths,
        graph_content_width,
        weak,
    } = context;
    // 标签配色跟随该 commit 的 lane 颜色，与圆点/连线呼应。
    let accent = palette[row.layout.dot_color % palette.len()];
    let layout = row.layout.clone();
    let click_weak = weak.clone();
    div()
        .id(("git-graph-row", index))
        .h(row_height)
        .w_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .when(selected, |row| row.bg(colors.element_selected))
        .hover(|style| style.bg(colors.element_hover))
        .on_click(move |_event, _window, cx| {
            if let Some(view) = click_weak.upgrade() {
                view.update(cx, |view, cx| {
                    view.selected = Some(index);
                    cx.notify();
                });
            }
        })
        .child(
            graph_column(column_widths[0], &colors, true)
                .flex()
                .justify_center()
                .overflow_hidden()
                .child(
                    canvas(
                        |_bounds, _window, _cx| {},
                        move |bounds, _state, window, _cx| {
                            paint_graph(bounds, &layout, &palette, window);
                        },
                    )
                    .w(graph_content_width)
                    .h(row_height),
                ),
        )
        .child(render_commit_column(
            &row.commit,
            &colors,
            accent,
            column_widths[1],
        ))
        .child(render_author_column(
            &row.commit.author_name,
            &row.commit.oid,
            &colors,
            column_widths[2],
        ))
        .child(render_time_column(
            row.commit.timestamp,
            &row.commit.oid,
            &colors,
            column_widths[3],
        ))
        .child(render_sha_column(
            &row.commit.oid,
            &colors,
            column_widths[4],
        ))
}

/// 计算所有行共用的 lane 列宽，避免不同提交的 lane 数量改变其他列的起始位置。
fn graph_column_width(rows: &[GraphRow]) -> Pixels {
    let max_lanes = rows
        .iter()
        .map(|row| row.layout.max_lanes)
        .max()
        .unwrap_or_default();
    max_lanes as f32 * LANE_WIDTH
}

/// 根据已加载内容计算默认列宽；默认值只受内容最大宽度限制。
fn content_fit_column_widths(
    rows: &[GraphRow],
    window: &Window,
    cx: &App,
) -> [Pixels; COLUMN_COUNT] {
    let ui_font = typography::ui_font();
    let content_font = typography::content_font();
    let font_size = typography_for_window(window, cx).content_size();
    let cell_padding = space::S2 * 2.0 + space::S1;
    let header_padding = space::S6 * 2.0 + space::S1;
    let labels = ["图形", "提交信息", "作者", "时间", "哈希"];
    let mut widths = labels.map(|label| {
        measure_text_width(window, label, ui_font.clone(), font_size) + header_padding
    });

    widths[0] = widths[0].max(graph_column_width(rows));
    for row in rows {
        let refs = parse_refs(&row.commit.refs);
        let refs_width: Pixels = refs
            .iter()
            .map(|reference| {
                measure_text_width(window, &reference.label, ui_font.clone(), font_size)
                    + space::S2 * 2.0
                    + space::S2
            })
            .sum();
        let ref_gaps = space::S8 * refs.len() as f32;
        let commit_width = space::S6
            + refs_width
            + ref_gaps
            + measure_text_width(window, &row.commit.subject, ui_font.clone(), font_size)
            + cell_padding;
        widths[1] = widths[1].max(commit_width);

        widths[2] = widths[2].max(
            measure_text_width(window, &row.commit.author_name, ui_font.clone(), font_size)
                + cell_padding,
        );
        let time = relative_time(row.commit.timestamp);
        widths[3] = widths[3]
            .max(measure_text_width(window, &time, ui_font.clone(), font_size) + cell_padding);
        let short_sha = row.commit.oid.get(..7).unwrap_or(&row.commit.oid);
        widths[4] = widths[4].max(
            measure_text_width(window, short_sha, content_font.clone(), font_size) + cell_padding,
        );
    }

    std::array::from_fn(|index| widths[index].min(COLUMN_CONTENT_MAX_WIDTHS[index]))
}

/// 使用与提交列表相同的字体和字号测量单行文本，显式换行时取最长一行。
fn measure_text_width(window: &Window, text: &str, font: Font, font_size: Pixels) -> Pixels {
    text.split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                return px(0.0);
            }
            let run = TextRun {
                len: line.len(),
                font: font.clone(),
                color: window.text_style().color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(line.to_owned().into(), font_size, &[run], None)
                .width
        })
        .max()
        .unwrap_or(px(0.0))
}

/// 调整相邻列的宽度：拖动边界只在两侧列之间重新分配空间，并保留两侧边界。
fn resize_column_widths(
    widths: &mut [Pixels; COLUMN_COUNT],
    boundary: usize,
    delta: Pixels,
) -> Pixels {
    if boundary + 1 >= COLUMN_COUNT {
        return px(0.0);
    }
    let left = widths[boundary];
    let right = widths[boundary + 1];
    let min_delta = (COLUMN_MIN_WIDTH - left).max(right - COLUMN_RESIZE_MAX_WIDTHS[boundary + 1]);
    let max_delta = (COLUMN_RESIZE_MAX_WIDTHS[boundary] - left).min(right - COLUMN_MIN_WIDTH);
    if min_delta > max_delta {
        return px(0.0);
    }
    let allowed_delta = delta.clamp(min_delta, max_delta);
    widths[boundary] = left + allowed_delta;
    widths[boundary + 1] = right - allowed_delta;
    allowed_delta
}

/// 将边界左侧的列恢复到内容适应宽度，并把宽度差交给右侧列。
fn reset_column_width(
    widths: &mut [Pixels; COLUMN_COUNT],
    boundary: usize,
    target_width: Pixels,
) -> Pixels {
    if boundary >= COLUMN_COUNT - 1 {
        return px(0.0);
    }
    resize_column_widths(widths, boundary, target_width - widths[boundary])
}

/// 表头：每个可见列都有边界线，边界处的热区负责启动拖拽调整。
fn render_graph_header(
    column_widths: [Pixels; COLUMN_COUNT],
    colors: ThemeColors,
    weak: &WeakEntity<GitGraphView>,
    row_height: Pixels,
) -> impl IntoElement {
    let labels = ["图形", "提交信息", "作者", "时间", "哈希"];
    let mut header = div()
        .id("git-graph-header")
        .w_full()
        .h(row_height)
        .flex()
        .items_center()
        .flex_none()
        .bg(colors.editor_background)
        .text_color(colors.text_placeholder)
        .border_b_1()
        .border_color(colors.border_variant);

    for (index, label) in labels.into_iter().enumerate() {
        let mut cell = graph_column(column_widths[index], &colors, index + 1 < COLUMN_COUNT)
            .relative()
            .px(space::S6)
            .child(label);

        if index + 1 < COLUMN_COUNT {
            let resize_weak = weak.clone();
            let resize_handle = div()
                .id(("git-graph-column-resize", index))
                .absolute()
                .top_0()
                .right_neg_0p5()
                .w(COLUMN_RESIZE_HANDLE_WIDTH)
                .h_full()
                .cursor_col_resize()
                .on_mouse_down(MouseButton::Left, move |event, _window, cx| {
                    if let Some(view) = resize_weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.begin_column_resize(index, event.position.x, cx);
                        });
                    }
                    cx.stop_propagation();
                })
                .on_click({
                    let reset_weak = weak.clone();
                    move |event, _window, cx| {
                        if event.click_count() >= 2
                            && let Some(view) = reset_weak.upgrade()
                        {
                            view.update(cx, |view, cx| view.reset_column(index, cx));
                        }
                        cx.stop_propagation();
                    }
                })
                .on_drag(DraggedGitGraphColumn(index), move |_, _, _, cx| {
                    cx.new(|_| DraggedGitGraphColumn(index))
                });
            cell = cell.child(resize_handle);
        }
        header = header.child(cell);
    }
    header
}

/// 提交信息列：分支/tag 标签与 subject 共用提交描述列，subject 在该列内截断。
fn render_commit_column(
    commit: &GraphCommit,
    colors: &ThemeColors,
    accent: Rgba,
    width: Pixels,
) -> gpui::Div {
    let subject = commit.subject.clone();
    let mut column = graph_column(width, colors, true)
        .min_w_0()
        .gap(space::S8)
        .pl(space::S6)
        .overflow_hidden()
        .border_r_1()
        .border_color(colors.border_variant);

    for reference in parse_refs(&commit.refs) {
        column = column.child(render_ref_chip(&reference, accent));
    }

    column.child(
        ButtonLike::new(format!("git-graph-subject-{}", commit.oid))
            .flex_grow()
            .padding(space::S2)
            .tooltip(column_tooltip(&subject))
            .on_right_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(subject.clone()));
            })
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .truncate()
                    .text_color(colors.text)
                    .child(commit.subject.clone()),
            ),
    )
}

/// 作者列：独立列宽，内容不足时通过 tooltip 查看完整值。
fn render_author_column(author: &str, oid: &str, colors: &ThemeColors, width: Pixels) -> gpui::Div {
    let author = author.to_string();
    graph_column(width, colors, true)
        .min_w_0()
        .overflow_hidden()
        .border_r_1()
        .border_color(colors.border_variant)
        .child(
            ButtonLike::new(format!("git-graph-author-{oid}"))
                .padding(space::S2)
                .tooltip(column_tooltip(&author))
                .on_right_click({
                    let author = author.clone();
                    move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(author.clone()));
                    }
                })
                .child(
                    div()
                        .overflow_hidden()
                        .truncate()
                        .text_color(colors.text_muted)
                        .child(author.clone()),
                ),
        )
}

/// 相对时间列：独立列宽，并为完整显示值提供 tooltip。
fn render_time_column(timestamp: i64, oid: &str, colors: &ThemeColors, width: Pixels) -> gpui::Div {
    let time = relative_time(timestamp);
    graph_column(width, colors, true)
        .min_w_0()
        .overflow_hidden()
        .border_r_1()
        .border_color(colors.border_variant)
        .child(
            ButtonLike::new(format!("git-graph-time-{oid}"))
                .padding(space::S2)
                .tooltip(column_tooltip(&time))
                .on_right_click({
                    let time = time.clone();
                    move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(time.clone()));
                    }
                })
                .child(
                    div()
                        .overflow_hidden()
                        .truncate()
                        .text_color(colors.text_muted)
                        .child(time),
                ),
        )
}

/// 短哈希列：展示短值，tooltip 保留完整 OID。
fn render_sha_column(oid: &str, colors: &ThemeColors, width: Pixels) -> gpui::Div {
    let oid = oid.to_string();
    let short_sha = oid.get(..7).unwrap_or(&oid).to_string();
    graph_column(width, colors, false)
        .min_w_0()
        .overflow_hidden()
        .child(
            ButtonLike::new(format!("git-graph-sha-{oid}"))
                .padding(space::S2)
                .tooltip(column_tooltip(&oid))
                .on_right_click({
                    let oid = oid.clone();
                    move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(oid.clone()));
                    }
                })
                .child(
                    div()
                        .overflow_hidden()
                        .truncate()
                        .text_color(colors.text_disabled)
                        .font(typography::content_font())
                        .child(short_sha),
                ),
        )
}

/// Git Graph 列单元格的通用提示：第一行是完整值，第二行说明右键复制行为。
fn column_tooltip(value: &str) -> TooltipSpec {
    TooltipSpec::from_lines([value.to_string(), "右键复制该列信息".to_string()])
}

/// 标签类型：决定 chip 的配色（当前分支高亮，tag 弱化，普通分支常规）。
enum RefKind {
    Head,
    Tag,
    Branch,
}

struct CommitRef {
    label: String,
    kind: RefKind,
}

/// 解析 `%D` 分段：`HEAD -> main`（当前分支）、`HEAD`（游离）、`tag: v1.0`（标签）、其余为分支。
fn parse_refs(refs: &[String]) -> Vec<CommitRef> {
    refs.iter()
        .filter_map(|raw| {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            if let Some(name) = raw.strip_prefix("tag: ") {
                Some(CommitRef {
                    label: name.to_string(),
                    kind: RefKind::Tag,
                })
            } else if let Some(branch) = raw.strip_prefix("HEAD -> ") {
                Some(CommitRef {
                    label: branch.to_string(),
                    kind: RefKind::Head,
                })
            } else if raw == "HEAD" {
                Some(CommitRef {
                    label: "HEAD".to_string(),
                    kind: RefKind::Head,
                })
            } else {
                Some(CommitRef {
                    label: raw.to_string(),
                    kind: RefKind::Branch,
                })
            }
        })
        .collect()
}

/// 渲染单个分支/tag 标签：配色取自该 commit 的 lane 颜色 `accent`，用同色的半透明背景 + 边框 + 文字呈现，使标签与图中圆点/连线颜色呼应；
/// HEAD（当前分支）用更高的背景与边框不透明度突出。
fn render_ref_chip(reference: &CommitRef, accent: Rgba) -> gpui::Div {
    let (bg_alpha, border_alpha) = match reference.kind {
        RefKind::Head => (0.20, 0.70),
        RefKind::Tag => (0.10, 0.34),
        RefKind::Branch => (0.12, 0.42),
    };
    div()
        .flex_none()
        .p(space::S2)
        .rounded_sm()
        .border_1()
        .bg(accent.opacity(bg_alpha))
        .border_color(accent.opacity(border_alpha))
        .text_color(accent)
        .child(reference.label.clone())
}

/// 由 unix 时间戳与当前时间差计算中文相对时间（不引入时区/时间库依赖）。
fn relative_time(timestamp: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let diff = now - timestamp;
    if diff < MINUTE {
        "刚刚".to_string()
    } else if diff < HOUR {
        format!("{} 分钟前", diff / MINUTE)
    } else if diff < DAY {
        format!("{} 小时前", diff / HOUR)
    } else if diff < MONTH {
        format!("{} 天前", diff / DAY)
    } else if diff < YEAR {
        format!("{} 个月前", diff / MONTH)
    } else {
        format!("{} 年前", diff / YEAR)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use zcv_language::LanguageRegistry;

    use super::*;

    /// 行高必须跟随内容字号通道：内容字号放大后行高应变大。
    /// 若走错通道，`cmd-=`（workspace::IncreaseContentFontSize）对版本控制图没有任何可见效果。
    #[gpui::test]
    fn row_height_follows_content_font_size(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let original = f32::from(typography::content_size(cx));
            let baseline = row_height(typography::content_line(cx));

            typography::set_base_typography(cx, Some(original + 4.), None, None);
            let enlarged = row_height(typography::content_line(cx));
            // 临时调整基础字号，验证行高随字号变化；测试结束后立即还原。
            typography::set_base_typography(cx, Some(original), None, None);

            assert!(
                enlarged > baseline,
                "内容字号 {original} → {} 应放大行高，实际 {baseline:?} → {enlarged:?}",
                original + 4.
            );
            assert_eq!(
                row_height(typography::content_line(cx)),
                baseline,
                "还原内容字号后行高应回到原值（字号以 f32 存储，往返无损）"
            );
        });
    }

    #[test]
    fn column_resize_preserves_total_width_and_minimums() {
        let mut widths = [px(140.0), px(400.0), px(100.0), px(120.0), px(100.0)];
        let total_before: f32 = widths.iter().map(|width| f32::from(*width)).sum();

        resize_column_widths(&mut widths, 0, px(-500.0));

        let total_after: f32 = widths.iter().map(|width| f32::from(*width)).sum();
        assert!((total_before - total_after).abs() < f32::EPSILON);
        assert_eq!(widths[0], COLUMN_MIN_WIDTH);
        assert_eq!(widths[1], px(524.0));
    }

    #[test]
    fn graph_column_width_uses_only_actual_lane_count() {
        let row = |max_lanes| GraphRow {
            commit: GraphCommit {
                oid: String::new(),
                parents: Vec::new(),
                author_name: String::new(),
                timestamp: 0,
                subject: String::new(),
                refs: Vec::new(),
            },
            layout: GraphRowLayout {
                dot_lane: 0,
                dot_color: 0,
                lines: Vec::new(),
                max_lanes,
            },
        };

        assert_eq!(graph_column_width(&[row(1)]), px(16.0));
        assert_eq!(graph_column_width(&[row(1), row(3)]), px(48.0));
        assert_eq!(graph_column_width(&[]), px(0.0));
    }

    #[test]
    fn column_resize_respects_maximums_on_both_sides() {
        let mut widths = [px(100.0), px(500.0), px(100.0), px(120.0), px(100.0)];
        resize_column_widths(&mut widths, 0, px(500.0));
        assert_eq!(widths[0], COLUMN_RESIZE_MAX_WIDTHS[0]);
        assert_eq!(widths[1], px(280.0));

        let mut widths = [px(200.0), px(700.0), px(100.0), px(120.0), px(100.0)];
        resize_column_widths(&mut widths, 0, px(-500.0));
        assert_eq!(widths[0], px(100.0));
        assert_eq!(widths[1], COLUMN_RESIZE_MAX_WIDTHS[1]);
    }

    #[test]
    fn column_resize_ignores_an_invalid_existing_pair() {
        let mut widths = [px(8.0), px(8.0), px(100.0), px(120.0), px(100.0)];
        assert_eq!(resize_column_widths(&mut widths, 0, px(10.0)), px(0.0));
        assert_eq!(widths[0], px(8.0));
        assert_eq!(widths[1], px(8.0));
    }

    #[test]
    fn double_click_reset_restores_left_column_and_preserves_total_width() {
        let mut widths = [px(220.0), px(420.0), px(120.0), px(112.0), px(96.0)];
        let total_before: f32 = widths.iter().map(|width| f32::from(*width)).sum();

        reset_column_width(&mut widths, 0, px(160.0));

        let total_after: f32 = widths.iter().map(|width| f32::from(*width)).sum();
        assert_eq!(widths[0], px(160.0));
        assert_eq!(widths[1], px(480.0));
        assert!((total_before - total_after).abs() < f32::EPSILON);
    }

    /// 回归：提交图视图与其搜索栏之间不得互相强引用。
    ///
    /// C4 后视图持有 SearchBar，SearchBar 又强持有视图作为搜索目标，构成环；
    /// 关闭标签/面板（不触发活动 Item 变化、因而不会清 target）时两者都无法释放。
    /// 目标改为弱句柄后，释放外部强引用即可让视图与搜索栏一起释放。
    #[gpui::test]
    fn git_graph_view_and_search_bar_release_together(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let project = cx.new(|cx| {
            Project::new(
                directory.path().to_path_buf(),
                Arc::new(LanguageRegistry::new()),
                cx,
            )
        });
        let view = cx.new(|cx| GitGraphView::new(project, cx));
        let search_bar = cx.read_entity(&view, |view, _| view.search_bar.clone());
        let weak_view = view.downgrade();
        let weak_search_bar = search_bar.downgrade();

        // 模拟工具项激活：搜索栏把视图登记为自搜索目标。
        let (_, visual) = cx.add_window_view(|window, cx| {
            let toolbar = cx.new(|_| GitGraphToolbar::new());
            toolbar.update(cx, |toolbar, cx| {
                toolbar.set_active_pane_item(Some(&view as &dyn ItemHandle), window, cx);
            });
            gpui::Empty
        });
        visual.run_until_parked();

        drop(search_bar);
        drop(view);
        // 实体释放分多轮 effect 完成：逐轮刷新直到视图与搜索栏都被回收。
        for _ in 0..4 {
            visual.update(|_, _| {});
            visual.run_until_parked();
        }

        assert!(
            weak_view.upgrade().is_none(),
            "提交图视图在外部强引用释放后应被回收（不再被搜索栏强持有）"
        );
        assert!(
            weak_search_bar.upgrade().is_none(),
            "搜索栏应随视图一起释放"
        );
    }
}
