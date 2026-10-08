//! 顶栏全局搜索入口：当前提供项目文件的快速打开。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    App, Context, ListAlignment, ListOffset, ListSizingBehavior, ListState, MouseButton, Pixels,
    Render, Subscription, Task, WeakEntity, Window, deferred, div, list, prelude::*, px,
};
use zcv_actions::{
    MoveDown, MoveUp, PickerCancel, PickerConfirm, PickerSelectNext, PickerSelectPrev,
};
use zcv_fuzzy::{Matcher, PathMatchScore};
use zcv_project::{Project, ProjectEvent};
use zcv_theme::{color, scale};
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
}

fn ranked_matches(files: &[FileCandidate], query: &str) -> Vec<usize> {
    let Some(mut matcher) = Matcher::new(query) else {
        return Vec::new();
    };
    let mut ranked: Vec<_> = files
        .iter()
        .enumerate()
        .filter_map(|(index, file)| {
            matcher
                .score_path(&file.name, &file.relative)
                .map(|score| (score, index))
        })
        .collect();
    let compare = |left: &(PathMatchScore, usize), right: &(PathMatchScore, usize)| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| files[left.1].relative.cmp(&files[right.1].relative))
    };
    if ranked.len() > MAX_RESULTS {
        ranked.select_nth_unstable_by(MAX_RESULTS, compare);
        ranked.truncate(MAX_RESULTS);
    }
    ranked.sort_unstable_by(compare);
    ranked
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, index)| index)
        .collect()
}

struct FileSearchState {
    on_open: OnFileOpen,
    files: Vec<FileCandidate>,
    matches: Vec<usize>,
    query: String,
    selected_index: usize,
    loading: bool,
}

impl FileSearchState {
    fn new(on_open: OnFileOpen) -> Self {
        Self {
            on_open,
            files: Vec::new(),
            matches: Vec::new(),
            query: String::new(),
            selected_index: 0,
            loading: false,
        }
    }

    fn set_files(&mut self, paths: Vec<PathBuf>, root: &Path) {
        self.files = paths
            .into_iter()
            .map(|path| FileCandidate::new(path, root))
            .collect();
        self.loading = false;
        self.update_filter();
    }

    fn update_filter(&mut self) {
        self.matches = ranked_matches(&self.files, &self.query);
        self.selected_index = 0;
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn update_matches(&mut self, query: String) {
        self.query = query;
        self.update_filter();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut App) {
        let Some(&index) = self.matches.get(self.selected_index) else {
            return;
        };
        let path = self.files[index].path.clone();
        (self.on_open)(path, window, cx);
    }

    fn render_match(&self, index: usize, selected: bool, cx: &App) -> gpui::AnyElement {
        let file = &self.files[self.matches[index]];
        ListItem::new(("global-search-file", index))
            .toggle_state(selected)
            .start_slot(SvgIcon::new("icons/file.svg").color(color::current(cx).icon_muted))
            .child(file.name.clone())
            .subtitle(file.relative.clone())
            .into_any_element()
    }
}

/// 搜索输入和结果由本组件持有；顶栏只决定它的布局位置。
pub(crate) struct GlobalSearch {
    input: Arc<dyn ErasedEditor>,
    state: FileSearchState,
    list_state: ListState,
    active: bool,
    project: gpui::Entity<Project>,
    _focus_subscription: Subscription,
    _blur_subscription: Subscription,
    _input_subscription: Subscription,
    _project_subscription: Subscription,
    scan_task: Option<Task<()>>,
}

impl GlobalSearch {
    pub(crate) fn new(
        project: gpui::Entity<Project>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let on_open: OnFileOpen = Box::new(move |path, window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace.open_path(path, true, window, cx)
                })
                .ok();
        });
        let factory = EDITOR_FACTORY
            .get()
            .expect("全局搜索需要 zcv_editor::init 注入编辑器工厂");
        let input = factory(cx);
        input.set_placeholder_text("搜索文件名或路径…", cx);
        let input_focus = input.focus_handle(cx);
        let focus_subscription = cx.on_focus(&input_focus, window, |search, _, cx| {
            search.active = true;
            search.refresh_files(cx);
            cx.notify();
        });
        let blur_subscription = cx.on_blur(&input_focus, window, |search, _, cx| {
            search.active = false;
            cx.notify();
        });
        let weak = cx.weak_entity();
        let input_subscription = input.subscribe(
            Box::new(move |ErasedEditorEvent::Edited, _, cx| {
                weak.update(cx, |search, cx| {
                    let query = search.input.text(cx);
                    search.state.update_matches(query);
                    search.matches_updated(cx);
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
            ) && search.active
            {
                search.refresh_files(cx);
            }
        });
        Self {
            input,
            state: FileSearchState::new(on_open),
            list_state: ListState::new(0, ListAlignment::Top, px(100.0)),
            active: false,
            project,
            _focus_subscription: focus_subscription,
            _blur_subscription: blur_subscription,
            _input_subscription: input_subscription,
            _project_subscription: project_subscription,
            scan_task: None,
        }
    }

    pub(crate) fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.input.focus_handle(cx), cx);
    }

    fn refresh_files(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.project.read(cx).root().map(Path::to_path_buf) else {
            self.scan_task.take();
            self.state.set_files(Vec::new(), Path::new(""));
            self.matches_updated(cx);
            return;
        };
        let scan = self.project.read(cx).searchable_file_paths(cx);
        self.state.set_files(Vec::new(), &root);
        self.state.loading = true;
        self.matches_updated(cx);
        self.scan_task = Some(cx.spawn(async move |search, cx| {
            let files = scan.await;
            search
                .update(cx, |search, cx| {
                    search.state.set_files(files, &root);
                    search.matches_updated(cx);
                })
                .ok();
        }));
    }

    fn matches_updated(&mut self, cx: &mut Context<Self>) {
        self.list_state.reset(self.state.match_count());
        self.list_state.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: Pixels::ZERO,
        });
        cx.notify();
    }

    fn select_next(&mut self, _: &PickerSelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.state.match_count();
        if count > 0 {
            self.state.selected_index = (self.state.selected_index + 1) % count;
            self.list_state
                .scroll_to_reveal_item(self.state.selected_index);
            cx.notify();
        }
    }

    fn select_prev(&mut self, _: &PickerSelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.state.match_count();
        if count > 0 {
            self.state.selected_index = (self.state.selected_index + count - 1) % count;
            self.list_state
                .scroll_to_reveal_item(self.state.selected_index);
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
        self.state.confirm(window, cx);
    }

    fn cancel(&mut self, _: &PickerCancel, window: &mut Window, cx: &mut Context<Self>) {
        window.blur(cx);
        self.active = false;
        cx.notify();
    }

    fn results(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let width = scale::to_pixels(SEARCH_WIDTH, window);
        let result_list = list(
            self.list_state.clone(),
            cx.processor(move |search, index, _, cx| {
                let entity = entity.clone();
                div()
                    .id(("global-search-result", index))
                    .w(width)
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |search, cx| {
                            search.state.selected_index = index;
                            search.state.confirm(window, cx);
                        });
                        cx.stop_propagation();
                    })
                    .child(search.state.render_match(
                        index,
                        index == search.state.selected_index,
                        cx,
                    ))
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
                .when(self.state.match_count() == 0, |el| {
                    el.child(
                        div()
                            .w_full()
                            .p(scale::S8)
                            .text_center()
                            .text_color(color::current(cx).text_placeholder)
                            .child("无搜索结果"),
                    )
                })
                .when(self.state.match_count() > 0, |el| el.child(result_list)),
            window,
            cx,
        )
    }
}

impl Render for GlobalSearch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = color::current(cx);
        div()
            .id("top-bar.global-search")
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
            .on_click(cx.listener(|search, _, window, cx| search.focus(window, cx)))
            .child(
                SvgIcon::new("icons/magnifying_glass.svg")
                    .size(window.rem_size())
                    .color(colors.icon_muted),
            )
            .child(div().flex_1().min_w_0().child(self.input.render()))
            .when(
                self.active && !self.state.query.trim().is_empty() && !self.state.loading,
                |el| el.child(self.results(window, cx)),
            )
    }
}

#[cfg(test)]
#[path = "test/global_search_tests.rs"]
mod tests;
