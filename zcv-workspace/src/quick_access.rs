//! 顶栏统一入口：普通访问聚合文件和命令，行列入口复用输入和结果面板。

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Context, DismissEvent, Entity, MouseButton, Pixels,
    Render, Subscription, Task, WeakEntity, Window, canvas, div, point, prelude::*, size,
};
use zcv_actions::commands;
use zcv_fuzzy::{MatchScore, Matcher};
use zcv_picker::{Picker, PickerDelegate};
use zcv_project::{Project, ProjectEvent, owning_workspace_root};
use zcv_theme::{FileIcons, color, fixed, scale};
use zcv_ui::{
    ListItem, OverlayAnchor, OverlayOutside, PopoverHost, PopoverLayer, SvgIcon, overlay,
};

use crate::{ItemHandle, Workspace, typography_for_window};

const SEARCH_WIDTH: gpui::DefiniteLength = scale::structural(420.0);
const MAX_RESULTS: usize = 100;

fn search_height(window: &Window, cx: &App) -> Pixels {
    typography_for_window(window, cx).ui_line() + scale::to_pixels(scale::S4, window) * 2.0
}

#[derive(Debug, Eq, PartialEq)]
struct FileCandidate {
    path: PathBuf,
    relative: String,
    name: String,
}

impl FileCandidate {
    fn new(path: PathBuf, roots: &[PathBuf]) -> Self {
        // 嵌套根命中多个时取最长前缀，相对路径最短。
        let relative = owning_workspace_root(&path, roots)
            .and_then(|root| path.strip_prefix(root).ok())
            .expect("项目候选文件必须位于某个工作区根下");
        let relative = relative.to_string_lossy().replace('\\', "/");
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
}

enum AccessMode {
    Mixed,
    LineColumn(Box<dyn ItemHandle>),
}

impl AccessMode {
    fn line_column_supported(&self, cx: &App) -> bool {
        match self {
            Self::Mixed => false,
            Self::LineColumn(target) => target.can_navigate_to_line_column(cx),
        }
    }
}

fn input_mode(query: &str) -> InputMode<'_> {
    let query = query.trim();
    if let Some(command) = query.strip_prefix('>') {
        InputMode::Commands(command.trim())
    } else if query.is_empty() {
        InputMode::Empty
    } else {
        InputMode::Mixed(query)
    }
}

fn parse_line_column(input: &str) -> Option<(usize, usize)> {
    fn positive_index(digits: &str) -> Option<usize> {
        let number = digits.bytes().fold(0usize, |number, digit| {
            number
                .saturating_mul(10)
                .saturating_add(usize::from(digit - b'0'))
        });
        number.checked_sub(1)
    }

    let mut numbers = input
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty());
    let line = positive_index(numbers.next()?)?;
    let column = numbers.next().map(positive_index).unwrap_or(Some(0))?;
    Some((line, column))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SearchResult {
    File(Arc<FileCandidate>),
    Command(usize),
    LineColumn { line: usize, column: usize },
}

fn ranked_results(
    files: &[Arc<FileCandidate>],
    commands: &[CommandCandidate],
    input: &str,
) -> Vec<SearchResult> {
    let mode = input_mode(input);
    let query = match mode {
        InputMode::Empty => return Vec::new(),
        InputMode::Mixed(query) | InputMode::Commands(query) => query,
    };
    let Some(mut matcher) = Matcher::new(query) else {
        return Vec::new();
    };
    let mut scored: Vec<(MatchScore, &str, SearchResult)> = Vec::new();
    if matches!(mode, InputMode::Mixed(_)) {
        scored.extend(files.iter().filter_map(|file| {
            file.match_score(&mut matcher).map(|score| {
                (
                    score,
                    file.relative.as_str(),
                    SearchResult::File(file.clone()),
                )
            })
        }));
    }
    scored.extend(commands.iter().enumerate().filter_map(|(index, command)| {
        command
            .match_score(&mut matcher)
            .map(|score| (score, command.action_name, SearchResult::Command(index)))
    }));
    let compare = |left: &(MatchScore, &str, SearchResult),
                   right: &(MatchScore, &str, SearchResult)| {
        right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1))
    };
    if scored.len() > MAX_RESULTS {
        scored.select_nth_unstable_by(MAX_RESULTS, compare);
        scored.truncate(MAX_RESULTS);
    }
    scored.sort_unstable_by(compare);
    scored.into_iter().map(|(_, _, result)| result).collect()
}

impl FileSearchState {
    fn new() -> Self {
        Self {
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

    fn set_files(&mut self, paths: Vec<PathBuf>, roots: &[PathBuf]) {
        self.files = paths
            .into_iter()
            .map(|path| Arc::new(FileCandidate::new(path, roots)))
            .collect();
        self.load_state = FileLoadState::Ready;
    }
}

/// 快速访问的业务数据；输入、列表和通用选择交互由 Picker 拥有。
struct QuickAccessDelegate {
    state: FileSearchState,
    workspace: WeakEntity<Workspace>,
    commands: Vec<CommandCandidate>,
    results: Vec<SearchResult>,
    selected_index: usize,
    mode: AccessMode,
    project: Entity<Project>,
    scan_task: Option<Task<()>>,
    _project_subscription: Subscription,
}

impl QuickAccessDelegate {
    fn new(
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Self {
        let project_subscription =
            cx.subscribe_in(&project, window, |picker, _, event, window, cx| {
                if matches!(
                    event,
                    ProjectEvent::FileSystemChanged
                        | ProjectEvent::WorktreeAdded { .. }
                        | ProjectEvent::WorktreeRemoved { .. }
                        | ProjectEvent::WorktreeRootChanged { .. }
                ) {
                    picker.delegate_mut().invalidate_files();
                    let query = picker.search_input().text(cx);
                    if picker.search_input().focus_handle(cx).is_focused(window)
                        && matches!(picker.delegate().mode, AccessMode::Mixed)
                        && matches!(input_mode(&query), InputMode::Mixed(_))
                    {
                        picker.refresh(cx);
                    }
                }
            });
        Self {
            state: FileSearchState::new(),
            workspace,
            commands: commands()
                .map(|metadata| CommandCandidate {
                    action_name: metadata.action_name,
                    title: metadata.title,
                    description: metadata.description,
                })
                .collect(),
            results: Vec::new(),
            selected_index: 0,
            mode: AccessMode::Mixed,
            project,
            scan_task: None,
            _project_subscription: project_subscription,
        }
    }

    fn invalidate_files(&mut self) {
        self.scan_task = None;
        self.state.invalidate();
    }

    fn start_scan(&mut self, cx: &mut Context<Picker<Self>>) {
        let roots: Vec<PathBuf> = self
            .project
            .read(cx)
            .roots()
            .map(Path::to_path_buf)
            .collect();
        if roots.is_empty() {
            self.state.set_files(Vec::new(), &[]);
            return;
        }
        let scan = self.project.read(cx).searchable_file_paths(cx);
        self.state.begin_scan();
        self.scan_task = Some(cx.spawn(async move |picker, cx| {
            let files = scan.await;
            let _ = picker.update(cx, |picker, cx| {
                let delegate = picker.delegate_mut();
                delegate.scan_task = None;
                delegate.state.set_files(files, &roots);
                picker.refresh(cx);
            });
        }));
    }
}

impl PickerDelegate for QuickAccessDelegate {
    fn match_count(&self) -> usize {
        self.results.len()
    }
    fn selected_index(&self) -> usize {
        self.selected_index
    }
    fn set_selected_index(&mut self, index: usize) {
        self.selected_index = index;
    }
    fn placeholder_text(&self) -> &str {
        ""
    }

    fn update_matches(&mut self, query: String, cx: &mut Context<Picker<Self>>) {
        if matches!(self.mode, AccessMode::Mixed)
            && matches!(input_mode(&query), InputMode::Mixed(_))
            && self.state.load_state == FileLoadState::Stale
        {
            self.start_scan(cx);
        }
        self.results = match &self.mode {
            AccessMode::Mixed => ranked_results(&self.state.files, &self.commands, &query),
            AccessMode::LineColumn(_) => {
                if !self.mode.line_column_supported(cx) {
                    Vec::new()
                } else {
                    parse_line_column(query.trim())
                        .map(|(line, column)| vec![SearchResult::LineColumn { line, column }])
                        .unwrap_or_default()
                }
            }
        };
        self.selected_index = 0;
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(result) = self.results.get(self.selected_index).cloned() else {
            return;
        };
        match result {
            SearchResult::File(file) => {
                let _ = self.workspace.update(cx, |workspace, cx| {
                    workspace.open_path(file.path.clone(), true, window, cx)
                });
            }
            SearchResult::Command(index) => {
                let Ok(action) = cx.build_action(self.commands[index].action_name, None) else {
                    return;
                };
                // 先结束选择会话并归还焦点，再把命令分发给其实际目标。
                cx.emit(DismissEvent);
                window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
            }
            SearchResult::LineColumn { line, column } => {
                if let AccessMode::LineColumn(target) = &self.mode
                    && target.navigate_to_line_column(line, column, cx)
                {
                    cx.emit(DismissEvent);
                }
            }
        }
    }

    fn render_empty(&self, query: &str, cx: &App) -> Option<AnyElement> {
        let text = match (&self.mode, input_mode(query)) {
            (AccessMode::LineColumn(_), _) if !self.mode.line_column_supported(cx) => {
                "该文件不支持跳转到指定行列"
            }
            (AccessMode::LineColumn(_), InputMode::Empty) => "输入行列数字，例如 42:8、第42行第8列",
            (AccessMode::LineColumn(_), _) => "请输入有效的行号或行列",
            (AccessMode::Mixed, InputMode::Empty) => "输入文件或命令 · > 仅搜索命令",
            (AccessMode::Mixed, InputMode::Commands("")) => "输入命令名称",
            (AccessMode::Mixed, InputMode::Mixed(_))
                if self.state.load_state == FileLoadState::Loading =>
            {
                "正在查找文件…"
            }
            _ => "无搜索结果",
        };
        Some(
            div()
                .w_full()
                .p(scale::S8)
                .text_color(color::current(cx).text_placeholder)
                .child(text)
                .into_any_element(),
        )
    }

    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _: &mut Context<Picker<Self>>,
    ) -> AnyElement {
        let item = match &self.results[index] {
            SearchResult::Command(command) => ListItem::new(("quick-access-command", index))
                .toggle_state(selected)
                .start_slot(SvgIcon::new("icons/chevron_right.svg"))
                .child(self.commands[*command].title)
                .subtitle(self.commands[*command].description),
            SearchResult::File(file) => ListItem::new(("quick-access-file", index))
                .toggle_state(selected)
                .start_slot(SvgIcon::new(FileIcons::get_icon(&file.path)))
                .child(file.name.clone())
                .subtitle(file.relative.clone()),
            SearchResult::LineColumn { line, column } => {
                ListItem::new(("quick-access-line-column", index))
                    .toggle_state(selected)
                    .start_slot(SvgIcon::new("icons/chevron_right.svg"))
                    .child(format!("跳转到第 {} 行，第 {} 列", line + 1, column + 1))
            }
        };
        item.into_any_element()
    }
}

/// 常驻顶栏入口及结果浮层的布局宿主；不持有输入、选中索引或列表状态。
pub(crate) struct QuickAccess {
    picker: Entity<Picker<QuickAccessDelegate>>,
    host: PopoverHost,
    workspace: WeakEntity<Workspace>,
    _focus_subscription: Subscription,
    _blur_subscription: Subscription,
    _picker_subscription: Subscription,
    _dismiss_subscription: Subscription,
}

impl QuickAccess {
    pub(crate) fn new(
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        picker_layer: PopoverLayer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = cx.new(|cx| {
            let delegate = QuickAccessDelegate::new(project, workspace.clone(), window, cx);
            Picker::new(delegate, scale::to_pixels(SEARCH_WIDTH, window), window, cx)
        });
        let input_focus = picker.read(cx).search_input().focus_handle(cx);
        let host = PopoverHost::new(input_focus.clone(), picker_layer);
        let focus_subscription = cx.on_focus(&input_focus, window, |search, _, cx| {
            search.picker.update(cx, |picker, cx| {
                let query = picker.search_input().text(cx);
                if matches!(picker.delegate().mode, AccessMode::Mixed)
                    && matches!(input_mode(&query), InputMode::Mixed(_))
                {
                    picker.delegate_mut().invalidate_files();
                }
                picker.refresh(cx);
            });
            cx.notify();
        });
        let blur_subscription = cx.on_blur(&input_focus, window, |search, _, cx| {
            search.host.clear();
            cx.notify();
        });
        let picker_subscription = cx.observe(&picker, |_, _, cx| cx.notify());
        let dismiss_subscription = cx.subscribe_in(
            &picker,
            window,
            |search, _, _: &DismissEvent, window, cx| {
                search.restore_focus(window, cx);
            },
        );
        Self {
            picker,
            host,
            workspace,
            _focus_subscription: focus_subscription,
            _blur_subscription: blur_subscription,
            _picker_subscription: picker_subscription,
            _dismiss_subscription: dismiss_subscription,
        }
    }

    pub(crate) fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker.update(cx, |picker, cx| {
            if !matches!(picker.delegate().mode, AccessMode::Mixed) {
                picker.delegate_mut().mode = AccessMode::Mixed;
                picker.set_query("", cx);
            }
        });
        self.host.activate(window, cx);
        window.focus(&self.picker.read(cx).search_input().focus_handle(cx), cx);
    }

    pub(crate) fn focus_line_column(
        &mut self,
        target: Box<dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            picker.delegate_mut().mode = AccessMode::LineColumn(target);
            picker.set_query("", cx);
        });
        self.host.activate(window, cx);
        window.focus(&self.picker.read(cx).search_input().focus_handle(cx), cx);
    }

    fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.host.dismiss(window, cx);
        self.picker.update(cx, |picker, cx| {
            picker.delegate_mut().mode = AccessMode::Mixed;
            picker.set_query("", cx);
        });
        if self
            .picker
            .read(cx)
            .search_input()
            .focus_handle(cx)
            .is_focused(window)
            && let Some(workspace) = self.workspace.upgrade()
        {
            workspace.update(cx, |workspace, cx| workspace.focus_center_pane(window, cx));
        }
        cx.notify();
    }

    fn results(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        input_bounds: Bounds<Pixels>,
    ) -> AnyElement {
        let width = input_bounds.size.width;
        let content = self.picker.update(cx, |picker, cx| {
            picker.set_width(width - fixed::HAIRLINE * 2.0);
            picker.render_results(window, cx)
        });
        let weak = cx.weak_entity();
        overlay(window)
            .anchor(OverlayAnchor::AtPosition(point(
                input_bounds.left(),
                input_bounds.bottom() + scale::to_pixels(scale::S2, window),
            )))
            .min_width(width)
            .outside(OverlayOutside::Passthrough(Box::new(
                move |event, window, cx| {
                    if input_bounds.contains(&event.position) {
                        return;
                    }
                    let _ = weak.update(cx, |search, cx| search.restore_focus(window, cx));
                },
            )))
            .child(
                div()
                    .w(width)
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .rounded_lg()
                    .border_1()
                    .border_color(color::current(cx).border)
                    .bg(color::current(cx).elevated_surface_background)
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(content),
            )
            .into_any_element()
    }
}

impl Render for QuickAccess {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.picker.update(cx, |picker, cx| picker.render_input(cx));
        let weak = cx.weak_entity();
        let colors = color::current(cx);
        div()
            .id("top-bar.quick-access")
            .debug_selector(|| "quick-access-input".into())
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
            .capture_any_mouse_down(cx.listener(|search, _, window, cx| {
                if search
                    .picker
                    .read(cx)
                    .search_input()
                    .focus_handle(cx)
                    .is_focused(window)
                {
                    search.host.activate(window, cx);
                } else {
                    search.focus(window, cx);
                }
            }))
            .on_click(cx.listener(|search, _, window, cx| {
                window.focus(&search.picker.read(cx).search_input().focus_handle(cx), cx);
                cx.stop_propagation();
            }))
            .child(
                SvgIcon::new("icons/magnifying_glass.svg")
                    .size(window.rem_size())
                    .color(colors.icon_muted),
            )
            .child(div().flex_1().min_w_0().child(input))
            .when(self.host.is_open(), |el| {
                el.child(
                    // 顶栏可压缩，浮层与列表必须使用本帧布局后的输入宽度。
                    canvas(
                        move |bounds, window, cx| {
                            // 绝对定位区域不含父容器边框，锚定时还原输入框外边界。
                            let bounds = bounds.dilate(fixed::HAIRLINE);
                            let mut results = weak
                                .update(cx, |search, cx| search.results(window, cx, bounds))
                                .expect("快速访问在当前帧必须存活");
                            results.prepaint_as_root(
                                point(Pixels::ZERO, Pixels::ZERO),
                                size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                                window,
                                cx,
                            );
                            results
                        },
                        |_, mut results, window, cx| results.paint(window, cx),
                    )
                    .absolute()
                    .inset_0(),
                )
            })
    }
}

#[cfg(test)]
#[path = "test/quick_access_tests.rs"]
mod tests;
