//! GitGraphView —— 只读图形化提交历史视图（workspace Item）。
//!
//! 数据来自 GitStore 的后台分批加载（`load_commit_graph`），lane 布局由 `zcv_git::GraphLayoutState` 逐行计算；
//! 视图侧只负责用 `gpui::canvas` 把每行的绘制指令画成圆点与连线，并渲染提交文本。
//! 与 `ProjectDiffView` 一致，通过 `deploy_at` 在 pane 中打开/复用，不做序列化持久化。

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, PathBuilder, Pixels,
    Render, Rgba, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, WeakEntity,
    Window, canvas, div, point, prelude::*, px, uniform_list,
};
use regex::RegexBuilder;
use zcv_actions::{
    Backtab, FindNext, FindPrevious, Tab, ToggleCaseSensitive, ToggleRegex, ToggleWholeWord,
};
use zcv_editor::{Editor, EditorEvent};
use zcv_git::{GraphCommit, GraphLayoutState, GraphLine, GraphRowLayout};
use zcv_project::{GitStoreEvent, Project};
use zcv_text::SearchQuery;
use zcv_theme::color::{self, ThemeColors};
use zcv_theme::{space, typography};
use zcv_ui::{Button, Scrollbar};
use zcv_workspace::{
    Direction, Item, SearchEvent, SearchableItem, SerializedItemProvider, SerializedPaneItem,
    Workspace,
};

// ── 布局常量（参考 Zed git_graph.rs） ────────────────────────────────

/// 单条 lane 的水平宽度。
const LANE_WIDTH: Pixels = px(16.0);
/// 提交圆点半径。
const CIRCLE_RADIUS: Pixels = px(3.5);
/// 连线线宽。
const LINE_WIDTH: Pixels = px(1.5);
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

pub(crate) struct GitGraphView {
    focus: FocusHandle,
    project: Entity<Project>,
    /// 已加载并完成布局的行（按 `git log` 顺序）。
    rows: Vec<GraphRow>,
    /// 跨批次承载 lane 占用状态，分批追加时复用同一实例。
    layout: GraphLayoutState,
    /// 下一批加载的游标：已加载的最后一条提交 oid（`None` 表示从 HEAD 开始）。
    cursor: Option<String>,
    loading: bool,
    /// 无更多提交（加载到空批或不足一批）。
    reached_end: bool,
    /// 当前选中行下标。
    selected: Option<usize>,
    search_matches: Vec<usize>,
    active_search_match: Option<usize>,
    scroll_handle: UniformListScrollHandle,
    scrollbar: Scrollbar<UniformListScrollHandle>,
    _git_subscription: Subscription,
    search_input: Entity<Editor>,
    _search_subscription: Subscription,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
}

impl GitGraphView {
    fn toggle_search_option(&mut self, option: u8, cx: &mut Context<Self>) {
        match option {
            0 => self.case_sensitive = !self.case_sensitive,
            1 => self.whole_word = !self.whole_word,
            2 => self.regex = !self.regex,
            _ => return,
        }
        let query = self.search_input.read(cx).text(cx);
        self.run_search(
            &SearchQuery {
                query,
                case_sensitive: self.case_sensitive,
                whole_word: self.whole_word,
                regex: self.regex,
            },
            cx,
        );
    }

    fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let scroll_handle = UniformListScrollHandle::default();
        let scrollbar = Scrollbar::vertical(scroll_handle.clone());
        let git_store = project.read(cx).git_store();
        let search_input = cx.new(Editor::single_line);
        search_input.update(cx, |editor, cx| {
            editor.set_placeholder_text("搜索提交…", cx);
        });
        let search_subscription =
            cx.subscribe(&search_input, |view, _input, event: &EditorEvent, cx| {
                if *event == EditorEvent::Edited {
                    let query = view.search_input.read(cx).text(cx);
                    let search_query = SearchQuery {
                        query,
                        case_sensitive: view.case_sensitive,
                        whole_word: view.whole_word,
                        regex: view.regex,
                    };
                    view.run_search(&search_query, cx);
                }
            });
        let git_subscription = cx.subscribe(&git_store, |view, _, event, cx| {
            if matches!(
                event,
                GitStoreEvent::Repositories | GitStoreEvent::ActiveRepositoryChanged
            ) && view.rows.is_empty()
            {
                view.reached_end = false;
                view.load_more(cx);
            }
        });
        let mut view = Self {
            focus,
            project,
            rows: Vec::new(),
            layout: GraphLayoutState::new(),
            cursor: None,
            loading: false,
            reached_end: false,
            selected: None,
            search_matches: Vec::new(),
            active_search_match: None,
            scroll_handle,
            scrollbar,
            _git_subscription: git_subscription,
            search_input,
            _search_subscription: search_subscription,
            case_sensitive: false,
            whole_word: false,
            regex: false,
        };
        if git_store.read(cx).is_repository_scan_ready() {
            view.load_more(cx);
        }
        view
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
        let cursor = self.cursor.clone();
        let load = git_store.read(cx).load_commit_graph(cursor, BATCH_SIZE);
        cx.spawn(async move |this, cx| {
            let commits = load.await;
            this.update(cx, |view, cx| {
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

    /// 把一批提交逐条布局后追加到 `rows`，并推进游标/到底标记。
    fn append_commits(&mut self, commits: Vec<GraphCommit>, cx: &mut Context<Self>) {
        if commits.is_empty() {
            self.reached_end = true;
            cx.notify();
            return;
        }
        let batch_len = commits.len();
        self.cursor = commits.last().map(|commit| commit.oid.clone());
        for commit in commits {
            let layout = self.layout.push(&commit);
            self.rows.push(GraphRow { commit, layout });
        }
        if batch_len < BATCH_SIZE {
            self.reached_end = true;
        }
        cx.notify();
    }
}

impl EventEmitter<SearchEvent> for GitGraphView {}

impl Focusable for GitGraphView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.search_input.read(cx).focus_handle()
    }
}

impl Render for GitGraphView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *color::current(cx);
        let palette = lane_palette(&colors);
        let row_height = row_height();
        let root = div()
            .debug_selector(|| "git-graph-view".into())
            .size_full()
            .track_focus(&self.focus)
            .key_context("GitGraphSearchBar")
            .on_action(cx.listener(|view, _: &FindNext, _, cx| {
                view.move_active_match(Direction::Next, 1, cx);
            }))
            .on_action(cx.listener(|view, _: &FindPrevious, _, cx| {
                view.move_active_match(Direction::Prev, 1, cx);
            }))
            .on_action(cx.listener(|view, _: &ToggleCaseSensitive, _, cx| {
                view.toggle_search_option(0, cx);
            }))
            .on_action(cx.listener(|view, _: &ToggleWholeWord, _, cx| {
                view.toggle_search_option(1, cx);
            }))
            .on_action(cx.listener(|view, _: &ToggleRegex, _, cx| {
                view.toggle_search_option(2, cx);
            }))
            .on_action(cx.listener(|view, _: &Tab, window, cx| {
                window.focus(&view.search_input.read(cx).focus_handle(), cx);
            }))
            .on_action(cx.listener(|view, _: &Backtab, window, cx| {
                window.focus(&view.search_input.read(cx).focus_handle(), cx);
            }))
            .bg(colors.editor_background)
            // 提交文本属于内容：字号走内容通道，字体族沿用 UI 比例字体，只有短 SHA 用等宽。
            .font(typography::ui_font())
            .text_size(typography::content_size())
            .text_color(colors.text);
        let search_bar = div()
            .w_full()
            .p(space::S6)
            .flex()
            .items_center()
            .gap(space::S6)
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h(space::S8)
                    .px(space::S6)
                    .flex()
                    .items_center()
                    .gap(space::S4)
                    .rounded_sm()
                    .border_1()
                    .border_color(colors.border)
                    .child(self.search_input.clone())
                    .child(
                        Button::icon("git-graph-case", "icons/case_sensitive.svg")
                            .label("区分大小写")
                            .shortcut(&ToggleCaseSensitive, cx)
                            .color(if self.case_sensitive {
                                colors.icon_accent
                            } else {
                                colors.text_muted
                            })
                            .on_click(
                                cx.listener(|view, _, _, cx| view.toggle_search_option(0, cx)),
                            ),
                    )
                    .child(
                        Button::icon("git-graph-word", "icons/whole_word.svg")
                            .label("整词匹配")
                            .shortcut(&ToggleWholeWord, cx)
                            .color(if self.whole_word {
                                colors.icon_accent
                            } else {
                                colors.text_muted
                            })
                            .on_click(
                                cx.listener(|view, _, _, cx| view.toggle_search_option(1, cx)),
                            ),
                    )
                    .child(
                        Button::icon("git-graph-regex", "icons/regex.svg")
                            .label("正则表达式")
                            .shortcut(&ToggleRegex, cx)
                            .color(if self.regex {
                                colors.icon_accent
                            } else {
                                colors.text_muted
                            })
                            .on_click(
                                cx.listener(|view, _, _, cx| view.toggle_search_option(2, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(space::S4)
                    .text_size(typography::ui_size() * 0.85)
                    .text_color(colors.text_muted)
                    .child(format!(
                        "{}/{}",
                        self.active_search_match.map_or(0, |index| index + 1),
                        self.search_matches.len()
                    )),
            )
            .child(
                Button::icon("git-graph-prev", "icons/chevron_left.svg")
                    .label("上一个匹配")
                    .shortcut(&FindPrevious, cx)
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.move_active_match(Direction::Prev, 1, cx);
                    })),
            )
            .child(
                Button::icon("git-graph-next", "icons/chevron_right.svg")
                    .label("下一个匹配")
                    .shortcut(&FindNext, cx)
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.move_active_match(Direction::Next, 1, cx);
                    })),
            );

        if self.rows.is_empty() {
            let message = if self.loading {
                "加载中…"
            } else {
                "暂无提交历史"
            };
            return root.child(search_bar).child(
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

        let has_query = !self.search_input.read(cx).text(cx).is_empty();
        let len = if !has_query {
            self.rows.len()
        } else {
            self.search_matches.len()
        };
        let weak = cx.weak_entity();
        let list = uniform_list("git-graph-list", len, move |range, _window, cx| {
            let Some(view) = weak.upgrade() else {
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
                    render_graph_row(&row, index, selected, colors, palette, row_height, &weak)
                        .into_any_element()
                })
                .collect()
        })
        .size_full()
        .track_scroll(&self.scroll_handle)
        .with_decoration(self.scrollbar.clone());

        root.child(search_bar).child(list)
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
                "query": self.search_input.read(cx).text(cx),
                "case_sensitive": self.case_sensitive,
                "whole_word": self.whole_word,
                "regex": self.regex,
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
    ) -> gpui::Task<anyhow::Result<Box<dyn zcv_workspace::ItemHandle>>> {
        let view = cx.new(|cx| GitGraphView::new(project, cx));
        let query = state
            .get("query")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let case_sensitive = state
            .get("case_sensitive")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let whole_word = state
            .get("whole_word")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let regex = state
            .get("regex")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        view.update(cx, |view, cx| {
            view.case_sensitive = case_sensitive;
            view.whole_word = whole_word;
            view.regex = regex;
            view.search_input
                .update(cx, |editor, cx| editor.set_text(query, cx));
        });
        gpui::Task::ready(Ok(Box::new(view) as Box<dyn zcv_workspace::ItemHandle>))
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
fn row_height() -> Pixels {
    typography::content_line() + space::S4
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

/// 单行图形区宽度：本行用到的 lane 跨度（逐行独立，形成经典阶梯状）。
fn graph_area_width(layout: &GraphRowLayout) -> Pixels {
    let mut max_lane = layout.dot_lane;
    for line in &layout.lines {
        let lane = match line {
            GraphLine::Pass { lane, .. } => *lane,
            GraphLine::MergeIn { from_lane, .. } => *from_lane,
            GraphLine::ForkOut { to_lane, .. } => *to_lane,
        };
        if lane > max_lane {
            max_lane = lane;
        }
    }
    (max_lane as f32 + 1.0) * LANE_WIDTH
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

/// 用两段半圆弧填充一个提交圆点（移植 Zed `draw_commit_circle`）。
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
fn render_graph_row(
    row: &GraphRow,
    index: usize,
    selected: bool,
    colors: ThemeColors,
    palette: [Rgba; 6],
    row_height: Pixels,
    weak: &WeakEntity<GitGraphView>,
) -> impl IntoElement {
    let graph_width = graph_area_width(&row.layout);
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
            canvas(
                |_bounds, _window, _cx| {},
                move |bounds, _state, window, _cx| {
                    paint_graph(bounds, &layout, &palette, window);
                },
            )
            .w(graph_width)
            .h(row_height)
            .flex_none(),
        )
        .child(render_text_area(&row.commit, &colors, accent))
}

/// 文本区：分支/tag 标签 + subject（占满并截断）+ 作者 + 相对时间 + 短 SHA。
fn render_text_area(commit: &GraphCommit, colors: &ThemeColors, accent: Rgba) -> gpui::Div {
    let mut text = div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(space::S8)
        .pl(space::S6)
        .pr(space::S12)
        .overflow_hidden();

    for reference in parse_refs(&commit.refs) {
        text = text.child(render_ref_chip(&reference, accent));
    }

    let short_sha = commit.oid.get(..7).unwrap_or(&commit.oid).to_string();
    text.child(
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .truncate()
            .text_color(colors.text)
            .child(commit.subject.clone()),
    )
    .child(
        div()
            .flex_none()
            .text_color(colors.text_muted)
            .child(commit.author_name.clone()),
    )
    .child(
        div()
            .flex_none()
            .text_color(colors.text_muted)
            .child(relative_time(commit.timestamp)),
    )
    .child(
        div()
            .flex_none()
            .text_color(colors.text_disabled)
            .font(typography::content_font())
            .child(short_sha),
    )
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
    use super::*;

    /// 行高必须跟随内容字号通道：内容字号放大后行高应变大。
    /// 若走错通道，`cmd-=`（workspace::IncreaseContentFontSize）对版本控制图没有任何可见效果。
    #[gpui::test]
    fn row_height_follows_content_font_size(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let original = f32::from(typography::content_size());
            let baseline = row_height();

            typography::set_typography(cx, Some(original + 4.), None, None);
            let enlarged = row_height();
            // 字号是进程级运行时状态：立即还原，避免影响并行执行的其他可视测试。
            typography::set_typography(cx, Some(original), None, None);

            assert!(
                enlarged > baseline,
                "内容字号 {original} → {} 应放大行高，实际 {baseline:?} → {enlarged:?}",
                original + 4.
            );
            assert_eq!(
                row_height(),
                baseline,
                "还原内容字号后行高应回到原值（字号以 f32 存储，往返无损）"
            );
        });
    }
}
