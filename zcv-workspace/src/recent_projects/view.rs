//! 最近项目入口与选择浮层。
//!
//! 顶栏入口显示当前项目，浮层展示可切换的最近项目。
//! 按钮内联在布局中，浮层用 deferred + anchored 逃逸。
//!
//! 最近项目从 `~/.zcv/recent_projects.json` 读取。

use std::{
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use gpui::{
    Action, AnyElement, App, ClickEvent, Context, Entity, Pixels, Render, Subscription, WeakEntity,
    Window, div, prelude::*, px,
};
use zcv_actions::{
    AddDirectoryToProject, DeleteRecentProject, OpenLocalDirectory, OpenLocalFile,
    ToggleRecentProjects,
};
use zcv_fuzzy::Matcher;
use zcv_keymap::{KeyBindings, display_shortcut};
use zcv_picker::{PICKER_WIDTH, Picker, PickerDelegate, picker_divider, toggle_picker};
use zcv_project::{GitStoreEvent, Project, ProjectEvent};
use zcv_theme::{color, scale, typography};
use zcv_ui::{
    Button, ButtonLike, ListItem, PopoverHost, PopoverLayer, SvgIcon, ToastKind, TooltipSpec,
};

use crate::{
    Workspace,
    recent_projects::{self, ProjectEntry},
    typography_for_window,
};

// ═══ 回调 ════════════════════════════════════════════════════════

/// 项目选中回调 —— 参数为工作区的有序根集合。
pub type OnProjectSelected = Rc<dyn Fn(Vec<PathBuf>, &mut Window, &mut App)>;

/// 最近项目向宿主上报用户可见错误的回调（由装配层决定 toast 呈现）。
type ErrorReporter = Rc<dyn Fn(String, &mut App)>;

/// 项目按钮中名称区域的最大宽度；完整项目列表由展开浮层提供。
const PROJECT_BUTTON_MAX_WIDTH: Pixels = px(190.0);

fn project_root_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

/// 项目按钮的显示基准：活动仓库所属根优先，其次首个根。
fn ordered_project_roots(project: &Project, cx: &App) -> Vec<PathBuf> {
    let roots = project.roots().map(Path::to_path_buf).collect::<Vec<_>>();
    let git_store = project.git_store();
    let active_root = git_store
        .read(cx)
        .active_repository_workdir()
        .and_then(|workdir| project.root_for_path(workdir))
        .map(Path::to_path_buf);
    let Some(active_root) = active_root else {
        return roots;
    };
    let mut ordered = Vec::with_capacity(roots.len());
    ordered.push(active_root.clone());
    ordered.extend(roots.into_iter().filter(|root| root != &active_root));
    ordered
}

/// 项目按钮的名称与固定后缀分开返回，名称单独截断，其余项目数量始终可见。
fn project_button_parts(project: &Project, cx: &App) -> (String, Option<String>) {
    let roots = ordered_project_roots(project, cx);
    let Some(first) = roots.first() else {
        return ("选择项目".to_owned(), None);
    };
    let name = project_root_name(first);
    let suffix = (roots.len() > 1).then(|| format!("...+{}", roots.len() - 1));
    (name, suffix)
}

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
        let roots = Self::current_roots(project, cx);
        if roots.is_empty() {
            return;
        }
        if !projects
            .iter()
            .any(|entry| Self::entry_matches_roots(entry, &roots))
        {
            projects.insert(
                0,
                ProjectEntry {
                    paths: roots
                        .iter()
                        .map(|root| root.to_string_lossy().into_owned())
                        .collect(),
                },
            );
        }
    }

    fn select_current_project(&mut self, cx: &App) {
        let roots = Self::current_roots(&self.project, cx);
        self.selected_index = self
            .filtered
            .iter()
            .position(|&index| Self::entry_matches_roots(&self.projects[index], &roots))
            .unwrap_or(0);
    }

    fn is_current_project(&self, entry: &ProjectEntry, cx: &App) -> bool {
        let roots = Self::current_roots(&self.project, cx);
        Self::entry_matches_roots(entry, &roots)
    }

    /// 当前工作区的根集合（有序）。
    fn current_roots(project: &Entity<Project>, cx: &App) -> Vec<PathBuf> {
        project.read(cx).roots().map(Path::to_path_buf).collect()
    }

    /// 记录是否与给定根集合一致（长度与逐根路径都相同）。
    fn entry_matches_roots(entry: &ProjectEntry, roots: &[PathBuf]) -> bool {
        entry.paths.len() == roots.len()
            && entry
                .paths
                .iter()
                .zip(roots.iter())
                .all(|(stored, root)| Path::new(stored) == root.as_path())
    }

    fn do_filter(&mut self) {
        if let Some(mut matcher) = Matcher::new(&self.query) {
            let mut ranked: Vec<_> = self
                .projects
                .iter()
                .enumerate()
                .filter_map(|(index, project)| {
                    matcher
                        .score_fields(&project.label(), &project.search_text())
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
        let paths = self.projects[project_ix].paths.clone();
        recent_projects::remove_from_recent(&paths)?;
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
        cb(entry.paths.iter().map(PathBuf::from).collect(), window, cx);
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
            .subtitle(
                div()
                    .flex()
                    .flex_col()
                    .children(entry.paths.iter().enumerate().map(|(path_index, path)| {
                        div()
                            .debug_selector(move || {
                                format!("recent-project-path-{index}-{path_index}")
                            })
                            .child(path.clone())
                    })),
            )
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
        "搜索..."
    }
    fn render_footer(&self, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let typography = typography_for_window(window, cx);
        let shortcut = cx
            .try_global::<KeyBindings>()
            .and_then(|kb| kb.display_shortcut_named(OpenLocalDirectory.name()));
        let directory_item = ListItem::new("open-local-directory").child("打开本地目录");
        let directory_item = if let Some(s) = shortcut {
            directory_item.end_slot(
                div()
                    .text_color(color::current(cx).text_placeholder)
                    .text_size(typography.ui_size())
                    .child(s),
            )
        } else {
            directory_item
        };
        Some(
            div()
                .child(picker_divider(cx))
                .child(
                    div()
                        .id("open-local-file-footer")
                        .debug_selector(|| "open-local-file-footer".into())
                        .child(ListItem::new("open-local-file").child("打开本地文件"))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(OpenLocalFile.boxed_clone(), cx);
                        }),
                )
                .child(
                    div()
                        .id("open-local-directory-footer")
                        .debug_selector(|| "open-local-directory-footer".into())
                        .child(directory_item)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(OpenLocalDirectory.boxed_clone(), cx);
                        }),
                )
                .child(
                    div()
                        .id("add-directory-to-project-footer")
                        .debug_selector(|| "add-directory-to-project-footer".into())
                        .child(ListItem::new("add-directory-to-project").child("添加目录到项目"))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(AddDirectoryToProject.boxed_clone(), cx);
                        }),
                )
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
    _subscriptions: Vec<Subscription>,
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
        let git_store = project.read(cx).git_store();
        let git_subscription = cx.subscribe(&git_store, |_, _, event, cx| {
            if matches!(
                event,
                GitStoreEvent::Repositories | GitStoreEvent::ActiveRepositoryChanged
            ) {
                cx.notify();
            }
        });
        let project_subscription = cx.subscribe(&project, |_, _, event, cx| {
            if matches!(
                event,
                ProjectEvent::WorktreeAdded { .. }
                    | ProjectEvent::WorktreeRemoved { .. }
                    | ProjectEvent::WorktreeRootChanged { .. }
            ) {
                cx.notify();
            }
        });
        let delegate = RecentProjectsDelegate::new(project, projects, on_selected, on_error, cx);

        let picker = cx.new(|cx| Picker::new(delegate, PICKER_WIDTH, window, cx));
        let host = PopoverHost::new(cx.focus_handle(), picker_layer);
        picker.update(cx, |picker, _| {
            picker.set_on_dismiss(host.on_dismiss_handler())
        });

        Self {
            host,
            picker,
            _subscriptions: vec![git_subscription, project_subscription],
        }
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

        let project = self.picker.read(cx).delegate().project.clone();
        let (button_name, button_suffix) = project_button_parts(project.read(cx), cx);
        let mut tooltip = TooltipSpec::from_lines(["最近项目"]);
        if let Some(shortcut) = display_shortcut(&ToggleRecentProjects, cx) {
            tooltip = tooltip.shortcut(shortcut);
        }

        let button = ButtonLike::new("recent-projects")
            .padding(scale::S2)
            .tooltip(tooltip)
            .on_click(cx.listener(|picker, _, window, cx| picker.toggle(window, cx)))
            .child(
                div()
                    .debug_selector(|| "recent-projects-label".into())
                    .max_w(PROJECT_BUTTON_MAX_WIDTH)
                    .min_w_0()
                    .flex()
                    .items_center()
                    .font(typography::ui_font())
                    .text_size(window.rem_size())
                    .line_height(typography::ui_line_at(window.rem_size(), cx))
                    .text_color(color_value)
                    .child(div().flex_1().min_w_0().truncate().child(button_name))
                    .when_some(button_suffix, |row, suffix| {
                        row.child(
                            div()
                                .debug_selector(|| "recent-projects-count".into())
                                .flex_none()
                                .child(suffix),
                        )
                    }),
            );

        let mut root = div()
            .track_focus(&self.host.focus_handle())
            // 复合 context 让 Picker 分组的快捷键与 Editor 同深度竞争
            .key_context("RecentProjects")
            .on_action(cx.listener(Self::handle_delete_recent))
            .relative()
            .child(
                div()
                    .debug_selector(|| "recent-projects-button".into())
                    .child(button),
            );

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
