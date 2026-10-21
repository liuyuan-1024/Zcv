//! 顶栏统一入口：聚合文件、命令与活动文档位置候选。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    App, Context, ListAlignment, ListOffset, ListSizingBehavior, ListState, MouseButton, Pixels,
    Render, Subscription, Task, WeakEntity, Window, deferred, div, list, prelude::*, px,
};
use zcv_actions::{
    MoveDown, MoveUp, PickerCancel, PickerConfirm, PickerSelectNext, PickerSelectPrev, commands,
};
use zcv_fuzzy::{MatchScore, Matcher};
use zcv_picker::{PickerHost, PickerLayer};
use zcv_project::{Project, ProjectEvent};
use zcv_theme::{FileIcons, color, scale};
use zcv_ui::{EDITOR_FACTORY, ErasedEditor, ErasedEditorEvent, ListItem, SvgIcon};

use crate::{Workspace, typography_for_window};

const SEARCH_WIDTH: gpui::DefiniteLength = scale::structural(420.0);
const MAX_RESULTS: usize = 100;
const RESULTS_MAX_HEIGHT: Pixels = px(420.0);

type OnFileOpen = Box<dyn Fn(PathBuf, &mut Window, &mut App)>;

fn search_height(window: &Window, cx: &App) -> Pixels {
    typography_for_window(window, cx).ui_line() + scale::to_pixels(scale::S4, window) * 2.0
}

fn positioned_results(content: impl IntoElement, window: &Window, cx: &App) -> impl IntoElement {
    deferred(
        div()
            .absolute()
            .top(search_height(window, cx))
            .left(Pixels::ZERO)
            .w_full()
            .child(content),
    )
    .with_priority(1)
}

#[derive(Debug, Eq, PartialEq)]
struct FileCandidate {
    path: PathBuf,
    relative: String,
    name: String,
}

impl FileCandidate {
    fn new(path: PathBuf, root: &Path) -> Self {
        let relative = path
            .strip_prefix(root)
            .expect("项目候选文件必须位于项目根目录")
            .to_string_lossy()
            .replace('\\', "/");
        let name = path
            .file_name()
            .expect("项目候选文件必须有文件名")
            .to_string_lossy()
            .into_owned();
        Self {
            path,
            relative,
            name,
        }
    }

    fn match_score(&self, matcher: &mut Matcher) -> Option<MatchScore> {
        matcher.score_fields(&self.name, &self.relative)
    }
}

struct CommandCandidate {
    action_name: &'static str,
    title: &'static str,
    description: &'static str,
}

impl CommandCandidate {
    fn match_score(&self, matcher: &mut Matcher) -> Option<MatchScore> {
        matcher.score_fields(self.title, self.action_name)
    }
}

struct FileSearchState {
    on_open: OnFileOpen,
    files: Vec<Arc<FileCandidate>>,
    load_state: FileLoadState,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum FileLoadState {
    Stale,
    Loading,
    Ready,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputMode<'a> {
    Empty,
    Mixed(&'a str),
    Commands(&'a str),
    LineColumn(&'a str),
}

fn input_mode(query: &str) -> InputMode<'_> {
    let query = query.trim();
    if let Some(command) = query.strip_prefix('>') {
        InputMode::Commands(command.trim())
    } else if let Some(position) = query.strip_prefix(':') {
        InputMode::LineColumn(position.trim())
    } else if query.is_empty() {
        InputMode::Empty
    } else {
        InputMode::Mixed(query)
    }
}

fn parse_line_column(input: &str) -> Option<(usize, usize)> {
    let (line, column) = input.split_once(':').unwrap_or((input, "1"));
    if line.is_empty() || column.contains(':') {
        return None;
    }
    let line = line.parse::<usize>().ok()?.checked_sub(1)?;
    let column = if column.is_empty() {
        0
    } else {
        column.parse::<usize>().ok()?.checked_sub(1)?
    };
    Some((line, column))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SearchResult {
    File(Arc<FileCandidate>),
    Command(usize),
    LineColumn { line: usize, column: usize },
}

impl SearchResult {
    fn sort_name<'a>(&'a self, commands: &'a [CommandCandidate]) -> &'a str {
        match self {
            Self::File(file) => &file.relative,
            Self::Command(index) => commands[*index].action_name,
            Self::LineColumn { .. } => unreachable!(),
        }
    }
}

fn ranked_results(
    files: &[Arc<FileCandidate>],
    commands: &[CommandCandidate],
    input: &str,
    has_active_document: bool,
) -> Vec<SearchResult> {
    let mode = input_mode(input);
    let query = match mode {
        InputMode::Empty => return Vec::new(),
        InputMode::Mixed(query) | InputMode::Commands(query) | InputMode::LineColumn(query) => {
            query
        }
    };
    let mut results = Vec::new();
    if has_active_document
        && matches!(mode, InputMode::Mixed(_) | InputMode::LineColumn(_))
        && let Some((line, column)) = parse_line_column(query)
    {
        results.push(SearchResult::LineColumn { line, column });
    }
    let Some(mut matcher) = Matcher::new(query) else {
        return results;
    };
    let mut scored: Vec<(MatchScore, SearchResult)> = Vec::new();
    if matches!(mode, InputMode::Mixed(_)) {
        scored.extend(files.iter().filter_map(|file| {
            file.match_score(&mut matcher)
                .map(|score| (score, SearchResult::File(file.clone())))
        }));
    }
    if matches!(mode, InputMode::Mixed(_) | InputMode::Commands(_)) {
        scored.extend(commands.iter().enumerate().filter_map(|(index, command)| {
            command
                .match_score(&mut matcher)
                .map(|score| (score, SearchResult::Command(index)))
        }));
    }
    let compare = |left: &(MatchScore, SearchResult), right: &(MatchScore, SearchResult)| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.sort_name(commands).cmp(right.1.sort_name(commands)))
    };
    let limit = MAX_RESULTS - results.len();
    if scored.len() > limit {
        scored.select_nth_unstable_by(limit, compare);
        scored.truncate(limit);
    }
    scored.sort_unstable_by(compare);
    results.extend(
        scored
            .into_iter()
            .map(|(_, result)| result)
            .take(MAX_RESULTS - results.len()),
    );
    results
}

impl FileSearchState {
    fn new(on_open: OnFileOpen) -> Self {
        Self {
            on_open,
            files: Vec::new(),
            load_state: FileLoadState::Stale,
        }
    }

    fn begin_scan(&mut self) {
        self.files.clear();
        self.load_state = FileLoadState::Loading;
    }

    fn invalidate(&mut self) {
        self.files.clear();
        self.load_state = FileLoadState::Stale;
    }

    fn set_files(&mut self, paths: Vec<PathBuf>, root: &Path) {
        self.files = paths
            .into_iter()
            .map(|path| Arc::new(FileCandidate::new(path, root)))
            .collect();
        self.load_state = FileLoadState::Ready;
    }

    fn confirm(&self, file: &FileCandidate, window: &mut Window, cx: &mut App) {
        let path = file.path.clone();
        (self.on_open)(path, window, cx);
    }
}

/// 搜索输入和结果由本组件持有；顶栏只决定它的布局位置。
pub(crate) struct QuickAccess {
    input: Arc<dyn ErasedEditor>,
    host: PickerHost,
    state: FileSearchState,
    commands: Vec<CommandCandidate>,
    results: Vec<SearchResult>,
    selected_index: usize,
    list_state: ListState,
    workspace: WeakEntity<Workspace>,
    project: gpui::Entity<Project>,
    _focus_subscription: Subscription,
    _blur_subscription: Subscription,
    _input_subscription: Subscription,
    _project_subscription: Subscription,
    scan_task: Option<Task<()>>,
}

impl QuickAccess {
    pub(crate) fn new(
        project: gpui::Entity<Project>,
        workspace: WeakEntity<Workspace>,
        picker_layer: PickerLayer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let workspace_for_open = workspace.clone();
        let on_open: OnFileOpen = Box::new(move |path, window, cx| {
            workspace_for_open
                .update(cx, |workspace, cx| {
                    workspace.open_path(path, true, window, cx)
                })
                .ok();
        });
        let factory = EDITOR_FACTORY
            .get()
            .expect("顶栏统一入口需要 zcv_editor::init 注入编辑器工厂");
        let input = factory(cx);
        let input_focus = input.focus_handle(cx);
        let host = PickerHost::new(input_focus.clone(), picker_layer);
        let focus_subscription = cx.on_focus(&input_focus, window, |search, _, cx| {
            if matches!(input_mode(&search.input.text(cx)), InputMode::Mixed(_)) {
                search.refresh_files(cx);
            } else {
                search.refresh_results(cx);
            }
            cx.notify();
        });
        let blur_subscription = cx.on_blur(&input_focus, window, |search, _, cx| {
            search.host.clear();
            cx.notify();
        });
        let weak = cx.weak_entity();
        let input_subscription = input.subscribe(
            Box::new(move |ErasedEditorEvent::Edited, _, cx| {
                weak.update(cx, |search, cx| {
                    let query = search.input.text(cx);
                    if matches!(input_mode(&query), InputMode::Mixed(_))
                        && search.state.load_state == FileLoadState::Stale
                    {
                        search.refresh_files(cx);
                    } else {
                        search.refresh_results(cx);
                    }
                })
                .ok();
            }),
            window,
            cx,
        );
        let project_subscription = cx.subscribe(&project, |search, _, event, cx| {
            if matches!(
                event,
                ProjectEvent::EntriesChanged | ProjectEvent::RootChanged(_)
            ) {
                search.scan_task.take();
                search.state.invalidate();
                if search.host.is_open()
                    && matches!(input_mode(&search.input.text(cx)), InputMode::Mixed(_))
                {
                    search.refresh_files(cx);
                }
            }
        });
        Self {
            input,
            host,
            state: FileSearchState::new(on_open),
            commands: commands()
                .map(|metadata| CommandCandidate {
                    action_name: metadata.action_name,
                    title: metadata.title,
                    description: metadata.description,
                })
                .collect(),
            results: Vec::new(),
            selected_index: 0,
            list_state: ListState::new(0, ListAlignment::Top, px(100.0)),
            workspace,
            project,
            _focus_subscription: focus_subscription,
            _blur_subscription: blur_subscription,
            _input_subscription: input_subscription,
            _project_subscription: project_subscription,
            scan_task: None,
        }
    }

    pub(crate) fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.host.activate(window, cx);
        window.focus(&self.input.focus_handle(cx), cx);
    }

    fn refresh_files(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.project.read(cx).root().map(Path::to_path_buf) else {
            self.scan_task.take();
            self.state.set_files(Vec::new(), Path::new(""));
            self.refresh_results(cx);
            return;
        };
        let scan = self.project.read(cx).searchable_file_paths(cx);
        self.state.begin_scan();
        self.refresh_results(cx);
        self.scan_task = Some(cx.spawn(async move |search, cx| {
            let files = scan.await;
            search
                .update(cx, |search, cx| {
                    search.state.set_files(files, &root);
                    search.refresh_results(cx);
                })
                .ok();
        }));
    }

    fn refresh_results(&mut self, cx: &mut Context<Self>) {
        let query = self.input.text(cx);
        let position = match input_mode(&query) {
            InputMode::Mixed(query) | InputMode::LineColumn(query) => parse_line_column(query),
            _ => None,
        };
        let has_active_document = position.is_some_and(|(line, column)| {
            self.workspace.upgrade().is_some_and(|workspace| {
                let pane = workspace.read(cx).pane().clone();
                pane.read(cx)
                    .active_item()
                    .is_some_and(|item| item.can_navigate_to_line_column(line, column, cx))
            })
        });
        self.results = ranked_results(
            &self.state.files,
            &self.commands,
            &query,
            has_active_document,
        );
        let count = self.results.len();
        self.selected_index = 0;
        self.list_state.reset(count);
        self.list_state.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: Pixels::ZERO,
        });
        cx.notify();
    }

    fn select_next(&mut self, _: &PickerSelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.results.len();
        if count > 0 {
            self.selected_index = (self.selected_index + 1) % count;
            self.list_state.scroll_to_reveal_item(self.selected_index);
            cx.notify();
        }
    }

    fn select_prev(&mut self, _: &PickerSelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.results.len();
        if count > 0 {
            self.selected_index = (self.selected_index + count - 1) % count;
            self.list_state.scroll_to_reveal_item(self.selected_index);
            cx.notify();
        }
    }

    fn editor_move_down(&mut self, _: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        self.select_next(&PickerSelectNext, window, cx);
    }

    fn editor_move_up(&mut self, _: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        self.select_prev(&PickerSelectPrev, window, cx);
    }

    fn confirm(&mut self, _: &PickerConfirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(result) = self.results.get(self.selected_index).cloned() else {
            return;
        };
        match result {
            SearchResult::File(file) => self.state.confirm(&file, window, cx),
            SearchResult::Command(index) => {
                let name = self.commands[index].action_name;
                let Ok(action) = cx.build_action(name, None) else {
                    return;
                };
                self.restore_focus(window, cx);
                window.dispatch_action(action, cx);
            }
            SearchResult::LineColumn { line, column } => {
                let navigated = self.workspace.upgrade().is_some_and(|workspace| {
                    let pane = workspace.read(cx).pane().clone();
                    pane.update(cx, |pane, cx| {
                        pane.active_item()
                            .is_some_and(|item| item.navigate_to_line_column(line, column, cx))
                    })
                });
                if navigated {
                    self.restore_focus(window, cx);
                }
            }
        }
    }

    fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.host.dismiss(window, cx);
        if self.input.focus_handle(cx).is_focused(window)
            && let Some(workspace) = self.workspace.upgrade()
        {
            workspace.update(cx, |workspace, cx| workspace.focus_center_pane(window, cx));
        }
    }

    fn cancel(&mut self, _: &PickerCancel, window: &mut Window, cx: &mut Context<Self>) {
        if !self.input.focus_handle(cx).is_focused(window) {
            return;
        }
        self.restore_focus(window, cx);
    }

    fn results(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let query = self.input.text(cx);
        let width = scale::to_pixels(SEARCH_WIDTH, window);
        let result_list = list(
            self.list_state.clone(),
            cx.processor(move |search, index, _, _| {
                let entity = entity.clone();
                let command_marker = SvgIcon::new("icons/chevron_right.svg");
                div()
                    .id(("quick-access-result", index))
                    .w(width)
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |search, cx| {
                            search.selected_index = index;
                            search.confirm(&PickerConfirm, window, cx);
                        });
                        cx.stop_propagation();
                    })
                    .child(match &search.results[index] {
                        SearchResult::Command(command) => {
                            ListItem::new(("quick-access-command", index))
                                .toggle_state(index == search.selected_index)
                                .start_slot(command_marker)
                                .child(search.commands[*command].title)
                                .subtitle(search.commands[*command].description)
                                .into_any_element()
                        }
                        SearchResult::File(file) => ListItem::new(("quick-access-file", index))
                            .toggle_state(index == search.selected_index)
                            .start_slot(SvgIcon::new(FileIcons::get_icon(&file.path)))
                            .child(file.name.clone())
                            .subtitle(file.relative.clone())
                            .into_any_element(),
                        SearchResult::LineColumn { line, column } => {
                            ListItem::new(("quick-access-line-column", index))
                                .toggle_state(index == search.selected_index)
                                .start_slot(command_marker)
                                .child(format!("跳转到第 {} 行，第 {} 列", line + 1, column + 1))
                                .into_any_element()
                        }
                    })
                    .into_any_element()
            }),
        )
        .with_sizing_behavior(ListSizingBehavior::Infer)
        .flex_grow(1.0)
        .min_h_0();
        positioned_results(
            div()
                .w_full()
                .max_h(RESULTS_MAX_HEIGHT.min(window.viewport_size().height))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded_lg()
                .border_1()
                .border_color(color::current(cx).border)
                .bg(color::current(cx).elevated_surface_background)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .when(self.results.is_empty(), |el| {
                    el.child(
                        div()
                            .w_full()
                            .p(scale::S8)
                            .text_color(color::current(cx).text_placeholder)
                            .child(match input_mode(&query) {
                                InputMode::Empty => {
                                    "输入文件、命令或行列 · > 仅搜索命令 · : 仅跳转行列".to_string()
                                }
                                InputMode::Commands("") => "输入命令名称".to_string(),
                                InputMode::LineColumn("") => {
                                    "输入行号或行号:列号，例如 42:8".to_string()
                                }
                                InputMode::LineColumn(_) => {
                                    "请输入有效的行号或行号:列号".to_string()
                                }
                                InputMode::Mixed(_)
                                    if self.state.load_state == FileLoadState::Loading =>
                                {
                                    "正在查找文件…".to_string()
                                }
                                _ => "无搜索结果".to_string(),
                            }),
                    )
                })
                .when(!self.results.is_empty(), |el| el.child(result_list)),
            window,
            cx,
        )
    }
}

impl Render for QuickAccess {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = color::current(cx);
        div()
            .id("top-bar.quick-access")
            .key_context("Picker")
            .relative()
            .flex()
            .items_center()
            .w_full()
            .max_w(SEARCH_WIDTH)
            .h(search_height(window, cx))
            .px(scale::S8)
            .gap(scale::S6)
            .rounded_sm()
            .border_1()
            .border_color(colors.border)
            .bg(colors.panel_background)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::editor_move_down))
            .on_action(cx.listener(Self::editor_move_up))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .capture_any_mouse_down(cx.listener(|search, _, window, cx| {
                search.host.activate(window, cx);
            }))
            .on_click(cx.listener(|search, _, window, cx| search.focus(window, cx)))
            .child(
                SvgIcon::new("icons/magnifying_glass.svg")
                    .size(window.rem_size())
                    .color(colors.icon_muted),
            )
            .child(div().flex_1().min_w_0().child(self.input.render()))
            .when(self.host.is_open(), |el| el.child(self.results(window, cx)))
    }
}

#[cfg(test)]
#[path = "test/quick_access_tests.rs"]
mod tests;
