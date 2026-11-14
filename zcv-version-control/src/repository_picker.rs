//! 版本控制面板的仓库选择器。
//!
//! 选择器只读取 GitStore 的仓库发现结果，并把用户选择交回面板；
//! 活动仓库的权威状态由 GitStore 保管，本组件不维护副本。

use std::rc::Rc;

use gpui::{App, Context, Entity, Render, Subscription, Window, div, prelude::*};
use zcv_fuzzy::Matcher;
use zcv_path::AbsolutePathBuf;
use zcv_picker::{PICKER_WIDTH, Picker, PickerDelegate, toggle_picker};
use zcv_project::{GitStore, GitStoreEvent};
use zcv_theme::color;
use zcv_ui::{Button, ListItem, PopoverHost, PopoverLayer, SvgIcon};

pub(crate) type OnRepositorySelected = Rc<dyn Fn(AbsolutePathBuf, &mut Window, &mut App)>;

#[derive(Clone)]
struct RepositoryChoice {
    workdir: AbsolutePathBuf,
    name: String,
    path: String,
}

impl RepositoryChoice {
    fn new(workdir: AbsolutePathBuf) -> Self {
        let path = workdir.as_path().display().to_string();
        let name = workdir
            .as_path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        Self {
            workdir,
            name,
            path,
        }
    }
}

struct RepositoryPickerDelegate {
    repositories: Vec<RepositoryChoice>,
    filtered: Vec<usize>,
    selected_index: usize,
    active: Option<AbsolutePathBuf>,
    on_select: OnRepositorySelected,
}

impl RepositoryPickerDelegate {
    fn new(
        repositories: Vec<RepositoryChoice>,
        active: Option<AbsolutePathBuf>,
        on_select: OnRepositorySelected,
    ) -> Self {
        let mut delegate = Self {
            repositories,
            filtered: Vec::new(),
            selected_index: 0,
            active,
            on_select,
        };
        delegate.reload_filter("");
        delegate
    }

    fn reload(
        &mut self,
        repositories: Vec<RepositoryChoice>,
        active: Option<AbsolutePathBuf>,
        query: &str,
    ) {
        self.repositories = repositories;
        self.active = active;
        self.reload_filter(query);
    }

    fn reload_filter(&mut self, query: &str) {
        self.filtered = filtered_indices(&self.repositories, query);
        self.selected_index = self
            .active
            .as_ref()
            .and_then(|workdir| {
                self.filtered.iter().position(|index| {
                    self.repositories[*index].workdir.as_path() == workdir.as_path()
                })
            })
            .unwrap_or(0);
    }

    fn selected_workdir(&self) -> Option<AbsolutePathBuf> {
        self.filtered
            .get(self.selected_index)
            .map(|index| self.repositories[*index].workdir.clone())
    }
}

fn filtered_indices(repositories: &[RepositoryChoice], query: &str) -> Vec<usize> {
    let Some(mut matcher) = Matcher::new(query) else {
        return (0..repositories.len()).collect();
    };
    let mut scored: Vec<_> = repositories
        .iter()
        .enumerate()
        .filter_map(|(index, repository)| {
            matcher
                .score_fields(&repository.name, &repository.path)
                .map(|score| (index, score))
        })
        .collect();
    scored.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_index.cmp(right_index))
    });
    scored.into_iter().map(|(index, _)| index).collect()
}

impl PickerDelegate for RepositoryPickerDelegate {
    fn match_count(&self) -> usize {
        self.filtered.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, index: usize) {
        self.selected_index = index;
    }

    fn update_matches(&mut self, query: String) {
        self.reload_filter(&query);
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(workdir) = self.selected_workdir() {
            (self.on_select)(workdir, window, cx);
        }
    }

    fn dismissed(&mut self) {}

    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _cx: &mut Context<Picker<Self>>,
    ) -> gpui::AnyElement {
        let choice = &self.repositories[self.filtered[index]];
        let is_active = self
            .active
            .as_ref()
            .is_some_and(|workdir| workdir.as_path() == choice.workdir.as_path());
        let item = ListItem::new(index)
            .toggle_state(selected)
            .child(choice.name.clone())
            .subtitle(choice.path.clone());
        if is_active {
            item.end_slot(
                SvgIcon::new("icons/check.svg")
                    .size(gpui::px(12.0))
                    .color(color::current(_cx).icon_accent),
            )
            .into_any_element()
        } else {
            item.into_any_element()
        }
    }
}

pub(crate) struct RepositoryPicker {
    host: PopoverHost,
    picker: Entity<Picker<RepositoryPickerDelegate>>,
    git_store: Entity<GitStore>,
    _git_subscription: Subscription,
}

impl RepositoryPicker {
    pub(crate) fn new(
        git_store: Entity<GitStore>,
        on_select: OnRepositorySelected,
        picker_layer: PopoverLayer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (repositories, active) = repository_choices(&git_store, cx);
        let delegate = RepositoryPickerDelegate::new(repositories, active, on_select);

        let picker = cx.new(|cx| Picker::new(delegate, PICKER_WIDTH, window, cx));
        let host = PopoverHost::new(cx.focus_handle(), picker_layer);
        picker.update(cx, |picker, _| {
            picker.set_on_dismiss(host.on_dismiss_handler())
        });

        let git_subscription =
            cx.subscribe(&git_store, |picker, store, event: &GitStoreEvent, cx| {
                if matches!(
                    event,
                    GitStoreEvent::Repositories | GitStoreEvent::ActiveRepositoryChanged
                ) {
                    let (repositories, active) = repository_choices(&store, cx);
                    if picker.host.is_open() {
                        let query = picker.picker.read(cx).search_input().text(cx);
                        picker.picker.update(cx, |picker, cx| {
                            picker.delegate_mut().reload(repositories, active, &query);
                            picker.matches_updated(cx);
                        });
                    } else {
                        picker.picker.update(cx, |picker, _| {
                            picker.delegate_mut().active = active;
                        });
                    }
                }
                cx.notify();
            });

        Self {
            host,
            picker,
            git_store,
            _git_subscription: git_subscription,
        }
    }

    fn display_name(&self, cx: &App) -> Option<String> {
        self.git_store.read(cx).active_repository_name()
    }

    pub(crate) fn toggle(&mut self, window: &mut Window, cx: &mut App) {
        if !self.host.is_open() {
            let (repositories, active) = repository_choices(&self.git_store, cx);
            self.picker.update(cx, |picker, cx| {
                picker.delegate_mut().reload(repositories, active, "");
                picker.set_query("", cx);
            });
        }
        toggle_picker(&mut self.host, &self.picker, window, cx);
    }
}

fn repository_choices(
    git_store: &Entity<GitStore>,
    cx: &App,
) -> (Vec<RepositoryChoice>, Option<AbsolutePathBuf>) {
    let store = git_store.read(cx);
    let repositories = store
        .repositories()
        .map(|(workdir, _)| {
            RepositoryChoice::new(
                AbsolutePathBuf::new(workdir.to_path_buf()).expect("仓库工作目录必须是绝对路径"),
            )
        })
        .collect();
    let active = store.active_repository_workdir().map(|workdir| {
        AbsolutePathBuf::new(workdir.to_path_buf()).expect("仓库工作目录必须是绝对路径")
    });
    (repositories, active)
}

impl Render for RepositoryPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(display_name) = self.display_name(cx) else {
            return div();
        };
        let color_value = if self.host.is_open() {
            color::current(cx).icon_accent
        } else {
            color::current(cx).text
        };
        let button = Button::icon_text(
            "version-control.repository",
            "icons/folder.svg",
            display_name,
        )
        .label("选择活动仓库")
        .color(color_value)
        .wrap_text()
        .on_click(cx.listener(|picker, _, window, cx| picker.toggle(window, cx)));

        let mut root = div()
            .track_focus(&self.host.focus_handle())
            .key_context("GitRepositorySelector")
            .relative()
            .child(button);
        if self.host.is_open() {
            root = root.child(self.host.overlay(_window, cx, self.picker.clone()));
        }
        root
    }
}
