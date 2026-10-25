//! 最近项目入口与选择浮层。
//!
//! 顶栏入口显示当前项目，浮层展示可切换的最近项目。
//! 按钮内联在布局中，浮层用 deferred + anchored 逃逸。
//!
//! 最近项目从 `~/.zcv/recent_projects.json` 读取。

use std::{path::Path, rc::Rc, time::Duration};

use gpui::{
    Action, AnyElement, App, ClickEvent, Context, Entity, Render, WeakEntity, Window, div,
    prelude::*,
};
use zcv_actions::{DeleteRecentProject, OpenLocalProject, ToggleRecentProjects};
use zcv_fuzzy::Matcher;
use zcv_keymap::{KeyBindings, display_shortcut};
use zcv_picker::{PICKER_WIDTH, Picker, PickerDelegate, picker_divider, toggle_picker};
use zcv_project::Project;
use zcv_theme::{color, scale};
use zcv_ui::{Button, ListItem, PopoverHost, PopoverLayer, SvgIcon};

use crate::{
    ToastKind, Workspace,
    recent_projects::{self, ProjectEntry},
    typography_for_window,
};

// ═══ 回调 ════════════════════════════════════════════════════════

/// 项目选中回调 —— 参数为项目路径。
pub type OnProjectSelected = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// 最近项目向宿主上报用户可见错误的回调（由装配层决定 toast 呈现）。
type ErrorReporter = Rc<dyn Fn(String, &mut App)>;

// ═══ 数据源 ═══════════════════════════════════════════════════════

/// 最近项目数据源。
struct RecentProjectsDelegate {
    project: Entity<Project>,
    query: String,
    projects: Vec<ProjectEntry>,
    filtered: Vec<usize>,
    selected_index: usize,
    on_selected: OnProjectSelected,
    on_error: ErrorReporter,
}

impl RecentProjectsDelegate {
    fn new(
        project: Entity<Project>,
        mut projects: Vec<ProjectEntry>,
        on_selected: OnProjectSelected,
        on_error: ErrorReporter,
        cx: &App,
    ) -> Self {
        Self::include_current_project(&mut projects, &project, cx);
        let filtered: Vec<usize> = (0..projects.len()).collect();
        let mut delegate = Self {
            project,
            query: String::new(),
            projects,
            filtered,
            selected_index: 0,
            on_selected,
            on_error,
        };
        delegate.select_current_project(cx);
        delegate
    }

    fn include_current_project(
        projects: &mut Vec<ProjectEntry>,
        project: &Entity<Project>,
        cx: &App,
    ) {
        if let Some(root) = project.read(cx).root()
            && !projects.iter().any(|entry| Path::new(&entry.path) == root)
        {
            projects.insert(
                0,
                ProjectEntry {
                    path: root.to_string_lossy().into_owned(),
                },
            );
        }
    }

    fn select_current_project(&mut self, cx: &App) {
        let root = self.project.read(cx).root();
        self.selected_index = self
            .filtered
            .iter()
            .position(|&index| root == Some(Path::new(&self.projects[index].path)))
            .unwrap_or(0);
    }

    fn is_current_project(&self, entry: &ProjectEntry, cx: &App) -> bool {
        self.project.read(cx).root() == Some(Path::new(&entry.path))
    }

    fn do_filter(&mut self) {
        if let Some(mut matcher) = Matcher::new(&self.query) {
            let mut ranked: Vec<_> = self
                .projects
                .iter()
                .enumerate()
                .filter_map(|(index, project)| {
                    matcher
                        .score_fields(&project.label(), &project.path)
                        .map(|score| (score, index))
                })
                .collect();
            ranked.sort_unstable_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
            self.filtered = ranked.into_iter().map(|(_, index)| index).collect();
        } else {
            self.filtered = (0..self.projects.len()).collect();
        }
        self.selected_index = self
            .selected_index
            .min(self.filtered.len().saturating_sub(1));
    }

    /// 移除 filtered 索引对应的最近项目；当前项目提示后保留，落盘失败时不改动内存列表。
    fn remove_project(&mut self, ix: usize, cx: &mut App) -> anyhow::Result<bool> {
        let project_ix = self.filtered[ix];
        if self.is_current_project(&self.projects[project_ix], cx) {
            (self.on_error)("不允许删除当前项目".into(), cx);
            return Ok(false);
        }
        let path = self.projects[project_ix].path.clone();
        recent_projects::remove_from_recent(&path)?;
        self.remove_project_in_memory(project_ix);
        Ok(true)
    }

    /// 从内存列表移除一个项目，并重算过滤结果与选中项。
    fn remove_project_in_memory(&mut self, project_ix: usize) {
        self.projects.remove(project_ix);
        self.do_filter();
    }

    /// 打开时重载最近项目、清空过滤并选中当前窗口的项目。
    fn reload_projects(&mut self, cx: &App) {
        self.projects = recent_projects::load_recent_projects();
        Self::include_current_project(&mut self.projects, &self.project, cx);
        self.query.clear();
        self.do_filter();
        self.select_current_project(cx);
    }
}

impl PickerDelegate for RecentProjectsDelegate {
    fn match_count(&self) -> usize {
        self.filtered.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize) {
        self.selected_index = ix;
    }

    fn update_matches(&mut self, query: String) {
        self.query = query;
        self.do_filter();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut App) {
        if self.filtered.is_empty() {
            return;
        }
        let entry = &self.projects[self.filtered[self.selected_index]];
        // 由回调（Workspace::switch_project）负责持久化最近项目
        let cb = self.on_selected.clone();
        cb(entry.path.clone(), window, cx);
    }

    fn dismissed(&mut self) {}

    fn render_match(
        &self,
        index: usize,
        is_selected: bool,
        cx: &mut Context<Picker<Self>>,
    ) -> AnyElement {
        let entry = &self.projects[self.filtered[index]];
        let is_current = self.is_current_project(entry, cx);
        let icon_color = color::current(cx).icon_muted;
        let on_error = self.on_error.clone();
        let remove = cx.listener(move |this, _: &ClickEvent, window, cx| {
            match this.delegate_mut().remove_project(index, cx) {
                Ok(true) => this.matches_updated(cx),
                Ok(false) => {}
                Err(error) => on_error(format!("删除最近项目失败：{error:#}"), cx),
            }
            window.refresh();
        });
        ListItem::new(index)
            .toggle_state(is_selected)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(scale::S6)
                    .text_color(color::current(cx).text)
                    .child(
                        div()
                            .debug_selector(move || format!("project-name-{index}"))
                            .child(entry.label()),
                    )
                    .when(is_current, |row| {
                        row.child(
                            div()
                                .debug_selector(|| "current-project-check".into())
                                .child(
                                    SvgIcon::new("icons/check.svg")
                                        .id(("current-project", index))
                                        .label("当前项目")
                                        .color(color::current(cx).icon_accent),
                                ),
                        )
                    }),
            )
            .subtitle(entry.path.clone())
            .when(!is_current, |row| {
                row.end_slot(
                    Button::icon(("delete-project", index), "icons/trash.svg")
                        .color(icon_color)
                        .label("移除")
                        .shortcut(display_shortcut(&DeleteRecentProject, cx))
                        .on_click(remove),
                )
            })
            .into_any_element()
    }

    fn placeholder_text(&self) -> &str {
        "搜索项目..."
    }
    fn render_footer(&self, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let typography = typography_for_window(window, cx);
        let shortcut = cx
            .try_global::<KeyBindings>()
            .and_then(|kb| kb.display_shortcut_named(OpenLocalProject.name()));
        let item = ListItem::new("open-local").child("打开本地项目");
        let item = if let Some(s) = shortcut {
            item.end_slot(
                div()
                    .text_color(color::current(cx).text_placeholder)
                    .text_size(typography.ui_size())
                    .child(s),
            )
        } else {
            item
        };
        Some(
            div()
                .id("open-local-footer")
                .child(picker_divider(cx))
                .child(item)
                .on_click(|_, window, cx| {
                    window.dispatch_action(OpenLocalProject.boxed_clone(), cx);
                })
                .into_any_element(),
        )
    }
}

// ═══ Entity ═════════════════════════════════════════════════════

/// 顶栏中的最近项目入口与选择浮层。
///
/// 显示当前项目名称，并提供最近项目切换入口。
pub struct RecentProjects {
    host: PopoverHost,
    picker: Entity<Picker<RecentProjectsDelegate>>,
}

impl RecentProjects {
    pub fn new(
        on_selected: OnProjectSelected,
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        picker_layer: PopoverLayer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let projects = recent_projects::load_recent_projects();
        let on_error: ErrorReporter = {
            let workspace = workspace.clone();
            Rc::new(move |message, cx| {
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.show_toast(
                            ToastKind::Error,
                            message,
                            vec![],
                            Some(Duration::from_secs(5)),
                            cx,
                        );
                    });
                }
            })
        };
        let delegate = RecentProjectsDelegate::new(project, projects, on_selected, on_error, cx);

        let picker = cx.new(|cx| Picker::new(delegate, PICKER_WIDTH, window, cx));
        let host = PopoverHost::new(cx.focus_handle(), picker_layer);
        picker.update(cx, |picker, _| {
            picker.set_on_dismiss(host.on_dismiss_handler())
        });

        Self { host, picker }
    }

    /// 外部切换（快捷键/按钮等）。
    pub fn toggle(&mut self, window: &mut Window, cx: &mut App) {
        if !self.host.is_open() {
            // 打开时从磁盘重新加载最近项目列表
            self.picker.update(cx, |picker, cx| {
                picker.delegate_mut().reload_projects(cx);
                picker.set_query("", cx);
            });
        }
        toggle_picker(&mut self.host, &self.picker, window, cx);
    }

    /// 删除当前选中的最近项目（快捷键 cmd-backspace，仅 picker 打开时绑定生效）。
    fn handle_delete_recent(
        &mut self,
        _: &DeleteRecentProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self.picker.update(cx, |picker, cx| {
            if picker.delegate().match_count() == 0 {
                return Ok(false);
            }
            let ix = picker.delegate().selected_index();
            let result = picker.delegate_mut().remove_project(ix, cx);
            if matches!(result, Ok(true)) {
                picker.matches_updated(cx);
            }
            result
        });
        if let Err(error) = result {
            let reporter = self.picker.read(cx).delegate().on_error.clone();
            reporter(format!("删除最近项目失败：{error:#}"), cx);
        }
        window.refresh();
    }
}

impl Render for RecentProjects {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let color_value = if self.host.is_open() {
            color::current(cx).icon_accent
        } else {
            color::current(cx).text
        };

        // 按钮上显示当前项目名称，没有时显示「选择项目」
        let button_text = self
            .picker
            .read(cx)
            .delegate()
            .project
            .read(cx)
            .root()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "选择项目".to_owned());

        let button = Button::text("recent-projects", button_text)
            .label("最近项目")
            .shortcut(display_shortcut(&ToggleRecentProjects, cx))
            .color(color_value)
            .on_click(cx.listener(|picker, _, window, cx| picker.toggle(window, cx)));

        let mut root = div()
            .track_focus(&self.host.focus_handle())
            // 复合 context 让 Picker 分组的快捷键与 Editor 同深度竞争
            .key_context("RecentProjects")
            .on_action(cx.listener(Self::handle_delete_recent))
            .relative()
            .child(button);

        // 浮层
        if self.host.is_open() {
            root = root.child(self.host.overlay(window, cx, self.picker.clone()));
        }

        root
    }
}

#[cfg(test)]
#[path = "../test/recent_projects_picker_tests.rs"]
mod tests;
