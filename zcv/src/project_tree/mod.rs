//! ProjectTree —— 项目文件树 Entity 组件。
//!
//! 持有 `Rc<RefCell<ProjectTreeState>>` 管理展开/选中状态和缓存行模型。
//! 文件系统只在刷新模型时读取；渲染与键盘导航只消费缓存。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use globset::{Glob, GlobSet, GlobSetBuilder};
use gpui::{
    App, Context, Div, Entity, KeyContext, MouseButton, ScrollStrategy, UniformListScrollHandle,
    WeakEntity, Window, actions, div, prelude::*, uniform_list,
};

use crate::project::Project;
use crate::settings::SettingsStore;
use crate::ui::tree;
use crate::workspace::{Panel, ToggleProjectTree};
use zcv_editor::Editor;
use zcv_git::{FileStatus, StatusCode};
use zcv_theme::color;

actions!(
    project_tree,
    [
        TreeSelectPrev,
        TreeSelectNext,
        TreeCollapse,
        TreeExpand,
        TreeActivate,
        TreeRename,
        TreeNewEntry,
        TreeTrash,
        TreeConfirmEdit,
        TreeCancelEdit
    ]
);

/// 打开文件回调
pub(crate) type OnOpenFile = Rc<dyn Fn(PathBuf, bool, &mut Window, &mut gpui::App)>;

/// 重命名文件或目录回调。
pub(crate) type OnRename = Rc<dyn Fn(PathBuf, PathBuf, &mut gpui::App) -> anyhow::Result<()>>;

/// 新建文件或目录回调。
pub(crate) type OnCreate = Rc<dyn Fn(PathBuf, bool, &mut gpui::App) -> anyhow::Result<()>>;

/// 将文件或目录移到系统废纸篓回调。
///
/// 带 `Window`：删除文件后需要关闭打开它的 tab，工具栏更新需要 window。
pub(crate) type OnTrash = Rc<dyn Fn(PathBuf, &mut Window, &mut gpui::App) -> anyhow::Result<()>>;

/// 查询当前可见行的 git 状态并更新行模型（目录行聚合、文件行精确）。
///
/// 由行集合变化的入口调用：展开/折叠、换根、git 事件。`cx` 只要求
/// `App`——鼠标点击闭包与 Entity 方法都可调用。
fn refresh_git_statuses_for_rows(
    state: &Rc<RefCell<ProjectTreeState>>,
    project: &Entity<Project>,
    cx: &mut gpui::App,
) {
    let rows: Vec<(PathBuf, bool)> = state
        .borrow()
        .rows
        .iter()
        .map(|row| (row.path.clone(), row.is_dir))
        .collect();
    let statuses = project.update(cx, |project, cx| {
        rows.iter()
            .filter_map(|(path, is_dir)| {
                let status = if *is_dir {
                    project.git_status_for_directory(path, cx)
                } else {
                    project
                        .git_status_for_path(path, cx)
                        .map(|entry| entry.status)
                };
                status.map(|status| (path.clone(), status))
            })
            .collect()
    });
    state.borrow_mut().update_git_statuses(statuses);
}

// ── Entity ──────────────────────────────────────────────────────────

pub(crate) struct ProjectTree {
    pub focus: gpui::FocusHandle,
    /// 当前项目根目录路径。
    root: PathBuf,
    /// git 状态查询（状态变化事件触发行颜色刷新）。
    project: Entity<Project>,
    state: Rc<RefCell<ProjectTreeState>>,
    scroll_handle: UniformListScrollHandle,
    entry_name_editor: Entity<Editor>,
    edit_state: Option<EditState>,
    on_open_file: Option<OnOpenFile>,
    on_rename: Option<OnRename>,
    on_create: Option<OnCreate>,
    on_trash: Option<OnTrash>,
}

impl ProjectTree {
    pub(crate) fn new(root: PathBuf, project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let entry_name_editor = cx.new(Editor::single_line);
        cx.observe(&entry_name_editor, |_, _, cx| cx.notify())
            .detach();
        let exclusions = SettingsStore::file_scan_exclusions(cx);
        let mut state = ProjectTreeState::new(root.clone());
        state.set_filter(&exclusions);
        // state::new 用空排除名单构建了初始行，按真实名单重建，
        state.refresh_rows();
        // git 状态变化（含忽略集变化）时刷新行颜色，不重扫目录。
        let git_store = project.read(cx).git_store();
        cx.subscribe(&git_store, |tree, _, _event, cx| {
            tree.refresh_git_statuses(cx);
        })
        .detach();
        Self {
            focus,
            root,
            project,
            state: Rc::new(RefCell::new(state)),
            scroll_handle: UniformListScrollHandle::default(),
            entry_name_editor,
            edit_state: None,
            on_open_file: None,
            on_rename: None,
            on_create: None,
            on_trash: None,
        }
    }

    /// 从 git 状态刷新行的忽略/颜色信息（git 事件驱动，不重扫目录）。
    ///
    /// 目录行取聚合状态（子项中优先级最高），文件行取精确状态。
    fn refresh_git_statuses(&mut self, cx: &mut Context<Self>) {
        refresh_git_statuses_for_rows(&self.state, &self.project, cx);
        cx.notify();
    }

    /// 设置打开文件的回调（由 Workspace 在创建后调用）。
    pub(crate) fn set_on_open_file(&mut self, callback: OnOpenFile) {
        self.on_open_file = Some(callback);
    }

    /// 设置重命名回调（由 Workspace 在创建后调用）。
    pub(crate) fn set_on_rename(&mut self, callback: OnRename) {
        self.on_rename = Some(callback);
    }

    /// 设置新建条目回调（由 Workspace 在创建后调用）。
    pub(crate) fn set_on_create(&mut self, callback: OnCreate) {
        self.on_create = Some(callback);
    }

    /// 设置删除（移到废纸篓）回调（由 Workspace 在创建后调用）。
    pub(crate) fn set_on_trash(&mut self, callback: OnTrash) {
        self.on_trash = Some(callback);
    }

    /// 更换项目根目录。
    pub(crate) fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root == root {
            return;
        }
        self.root = root.clone();
        let exclusions = SettingsStore::file_scan_exclusions(cx);
        self.state.borrow_mut().set_root(root.clone());
        self.state.borrow_mut().set_filter(&exclusions);
        self.state.borrow_mut().refresh_rows();
        // 行集合全变，git 状态缓存一并补齐（换根后缓存已清空）。
        self.refresh_git_statuses(cx);
        cx.notify();
    }

    /// 刷新行模型；同时从设置读取最新的扫描排除名单并重建过滤规则。
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let exclusions = SettingsStore::file_scan_exclusions(cx);
        self.state.borrow_mut().set_filter(&exclusions);
        self.state.borrow_mut().refresh_rows();
        self.refresh_git_statuses(cx);
        cx.notify();
    }

    /// 将活动文件标记并强制滚动到项目树中央。
    pub(crate) fn reveal_active_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let index = self.state.borrow_mut().reveal_active_path(path.as_deref());
        // reveal 会展开祖先目录产生新行，git 状态缓存需补齐。
        self.refresh_git_statuses(cx);
        if let Some(index) = index {
            self.scroll_handle
                .scroll_to_item_strict(index, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn rows_and_len(&self) -> Vec<ProjectTreeRow> {
        self.state.borrow().visible_rows().to_vec()
    }

    fn display_rows(&self, cx: &gpui::App) -> Vec<ProjectTreeRow> {
        let mut rows = self.rows_and_len();
        let Some(EditState {
            operation: EditOperation::Create { parent },
            ..
        }) = &self.edit_state
        else {
            return rows;
        };
        let Some((index, depth)) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| !row.is_new && &row.path == parent)
            .map(|(index, row)| (index, row.depth + 1))
        else {
            return rows;
        };
        rows.insert(
            index + 1,
            ProjectTreeRow {
                path: parent.clone(),
                name: String::new(),
                depth,
                is_dir: self.entry_name_editor.read(cx).text(cx).ends_with('/'),
                expanded: false,
                is_new: true,
                git_status: None,
            },
        );
        rows
    }

    fn dispatch_context(&self, window: &Window, cx: &Context<Self>) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        context.add("ProjectTree");
        context.add(
            if self
                .entry_name_editor
                .read(cx)
                .focus_handle()
                .is_focused(window)
            {
                "editing"
            } else {
                "not_editing"
            },
        );
        context
    }

    fn handle_tree_select_prev(
        &mut self,
        _: &TreeSelectPrev,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        let rows = self.rows_and_len();
        self.state.borrow_mut().select_up(&rows);
        window.refresh();
    }
    fn handle_tree_select_next(
        &mut self,
        _: &TreeSelectNext,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        let rows = self.rows_and_len();
        self.state.borrow_mut().select_down(&rows);
        window.refresh();
    }
    fn handle_tree_collapse(
        &mut self,
        _: &TreeCollapse,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        let mut state = self.state.borrow_mut();
        let rows = state.visible_rows().to_vec();
        let Some(idx) = state.selected_idx(&rows) else {
            return;
        };
        let row = &rows[idx];
        if row.is_dir && row.expanded {
            state.expanded.remove(&row.path);
            state.refresh_rows();
        } else if row.depth > 0 {
            let pd = row.depth - 1;
            if let Some(pi) = rows[..idx].iter().rposition(|r| r.is_dir && r.depth == pd) {
                state.selected = Some(rows[pi].path.clone());
            }
        }
        window.refresh();
    }
    fn handle_tree_expand(&mut self, _: &TreeExpand, window: &mut Window, cx: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let rows = state.visible_rows().to_vec();
        let Some(idx) = state.selected_idx(&rows) else {
            return;
        };
        let row = &rows[idx];
        if row.is_dir && !row.expanded {
            state.expanded.insert(row.path.clone());
            state.refresh_rows();
            // 展开产生新行：git 状态缓存需补齐（新行此前未查询过）。
            drop(state);
            self.refresh_git_statuses(cx);
            return;
        } else {
            state.select_down(&rows);
        }
        window.refresh();
    }
    /// 激活/预览选中行的共享逻辑：目录→展开/折叠；文件→打开。
    ///
    /// `focus_opened_item` 决定打开文件后是否把焦点交给编辑器：双击/键盘 enter 为 `true`（激活），鼠标单击为 `false`（预览，焦点留在项目树）。
    fn activate_selected(
        &mut self,
        focus_opened_item: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (path, is_dir) = {
            let state = self.state.borrow_mut();
            let rows = state.visible_rows();
            match state.selected_idx(rows) {
                Some(idx) => (Some(rows[idx].path.clone()), rows[idx].is_dir),
                None => (None, false),
            }
        };
        let Some(path) = path else {
            return;
        };
        self.state.borrow_mut().select(&path);
        if is_dir {
            self.state.borrow_mut().toggle_expand(&path);
            // 展开/折叠后行集合变化，git 状态缓存需补齐。
            refresh_git_statuses_for_rows(&self.state, &self.project, cx);
        } else if let Some(callback) = self.on_open_file.clone() {
            callback(path, focus_opened_item, window, cx);
        }
        window.refresh();
    }

    /// 激活选中行（打开文件并聚焦编辑器）。键盘 enter 与鼠标双击走这里。
    fn handle_tree_activate(
        &mut self,
        _: &TreeActivate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_selected(true, window, cx);
    }

    fn handle_tree_rename(&mut self, _: &TreeRename, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_state.is_some() {
            return;
        }
        let row = {
            let state = self.state.borrow();
            let rows = state.visible_rows();
            state.selected_idx(rows).map(|index| rows[index].clone())
        };
        let Some(row) = row else {
            return;
        };

        let selection_end = if row.is_dir {
            row.name.len()
        } else {
            Path::new(&row.name)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map_or(row.name.len(), str::len)
        };
        self.begin_edit(
            EditOperation::Rename {
                source: row.path,
                is_dir: row.is_dir,
            },
            &row.name,
            0..selection_end,
            window,
            cx,
        );
    }

    fn handle_tree_new_entry(
        &mut self,
        _: &TreeNewEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin_create(window, cx);
    }

    /// 将选中条目移到系统废纸篓；根目录行不可删除。
    fn handle_tree_trash(&mut self, _: &TreeTrash, window: &mut Window, cx: &mut Context<Self>) {
        let (path, index) = {
            let state = self.state.borrow();
            let rows = state.visible_rows();
            let Some(index) = state.selected_idx(rows) else {
                return;
            };
            (rows[index].path.clone(), index)
        };
        if path == self.root {
            return;
        }
        let Some(on_trash) = self.on_trash.clone() else {
            eprintln!("项目树删除失败：未配置项目删除服务");
            return;
        };
        if let Err(error) = on_trash(path, window, cx) {
            eprintln!("项目树删除失败：{error}");
            return;
        }
        let mut state = self.state.borrow_mut();
        state.refresh_rows();
        // 删除后选中原位置的下一个条目；删除的是最后一项时落在新的最后一项。
        let rows = state.visible_rows();
        if !rows.is_empty() {
            state.selected = Some(rows[index.min(rows.len() - 1)].path.clone());
        }
    }

    fn begin_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_state.is_some() {
            return;
        }
        let row = {
            let state = self.state.borrow();
            let rows = state.visible_rows();
            state.selected_idx(rows).map(|index| rows[index].clone())
        };
        let Some(row) = row else {
            return;
        };
        let parent = if row.is_dir {
            {
                let mut state = self.state.borrow_mut();
                state.expanded.insert(row.path.clone());
                state.refresh_rows();
            }
            // 展开父目录产生新行，git 状态缓存需补齐。
            self.refresh_git_statuses(cx);
            row.path
        } else {
            let Some(parent) = row.path.parent() else {
                return;
            };
            parent.to_path_buf()
        };
        self.begin_edit(EditOperation::Create { parent }, "", 0..0, window, cx);
    }

    fn begin_edit(
        &mut self,
        operation: EditOperation,
        name: &str,
        selection: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.entry_name_editor.update(cx, |editor, cx| {
            editor.set_text(name, cx);
            editor.select_byte_range(selection, cx);
        });
        self.edit_state = Some(EditState {
            operation,
            validation_error: None,
        });
        let focus = self.entry_name_editor.read(cx).focus_handle();
        window.focus(&focus);
        cx.notify();
    }

    fn handle_tree_confirm_edit(
        &mut self,
        _: &TreeConfirmEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(edit_state) = self.edit_state.clone() else {
            return;
        };
        let name = self.entry_name_editor.read(cx).text(cx);
        match edit_state.operation {
            EditOperation::Rename { source, .. } => {
                let destination = match rename_destination(&source, &name) {
                    Ok(destination) => destination,
                    Err(error) => return self.set_edit_error(error, cx),
                };
                if destination == source {
                    self.finish_edit(window, cx);
                    return;
                }
                let Some(on_rename) = self.on_rename.clone() else {
                    return self.set_edit_error(anyhow::anyhow!("未配置项目重命名服务"), cx);
                };
                if let Err(error) = on_rename(source.clone(), destination.clone(), cx) {
                    return self.set_edit_error(error, cx);
                }
                self.root = translate_path(&self.root, &source, &destination);
                self.state.borrow_mut().apply_rename(&source, &destination);
            }
            EditOperation::Create { parent } => {
                let new_entry = match new_entry_destination(&parent, &name) {
                    Ok(destination) => destination,
                    Err(error) => return self.set_edit_error(error, cx),
                };
                let Some(on_create) = self.on_create.clone() else {
                    return self.set_edit_error(anyhow::anyhow!("未配置项目新建服务"), cx);
                };
                if let Err(error) = on_create(new_entry.path.clone(), new_entry.is_dir, cx) {
                    return self.set_edit_error(error, cx);
                }
                let mut state = self.state.borrow_mut();
                let mut ancestor = new_entry.path.parent();
                while let Some(directory) = ancestor.filter(|path| path.starts_with(&parent)) {
                    state.expanded.insert(directory.to_path_buf());
                    if directory == parent {
                        break;
                    }
                    ancestor = directory.parent();
                }
                state.refresh_rows();
                state.selected = Some(new_entry.path.clone());
                drop(state);
                if !new_entry.is_dir
                    && let Some(on_open_file) = &self.on_open_file
                {
                    on_open_file(new_entry.path, true, window, cx);
                }
            }
        }
        self.finish_edit(window, cx);
    }

    fn handle_tree_cancel_edit(
        &mut self,
        _: &TreeCancelEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edit_state.is_some() {
            self.finish_edit(window, cx);
        }
    }

    fn set_edit_error(&mut self, error: anyhow::Error, cx: &mut Context<Self>) {
        eprintln!("项目树名称编辑失败：{error}");
        if let Some(edit_state) = &mut self.edit_state {
            edit_state.validation_error = Some(error.to_string());
        }
        cx.notify();
    }

    fn finish_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_state = None;
        window.focus(&self.focus);
        cx.notify();
    }
}

impl gpui::Render for ProjectTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        self.state.borrow_mut().ensure_selected();
        let display_rows = self.display_rows(cx);
        let len = display_rows.len();
        let is_focused = self.focus.contains_focused(window, cx);
        let render_context = ProjectTreeRenderContext {
            state: Rc::clone(&self.state),
            rows: display_rows.into(),
            focus: self.focus.clone(),
            weak: cx.weak_entity(),
            edit_state: self.edit_state.clone(),
            entry_name_editor: self.entry_name_editor.clone(),
        };

        div()
            .size_full()
            .track_focus(&self.focus)
            .key_context(self.dispatch_context(window, cx))
            .tab_index(0)
            .on_action(cx.listener(Self::handle_tree_select_prev))
            .on_action(cx.listener(Self::handle_tree_select_next))
            .on_action(cx.listener(Self::handle_tree_collapse))
            .on_action(cx.listener(Self::handle_tree_expand))
            .on_action(cx.listener(Self::handle_tree_activate))
            .on_action(cx.listener(Self::handle_tree_rename))
            .on_action(cx.listener(Self::handle_tree_new_entry))
            .on_action(cx.listener(Self::handle_tree_trash))
            .on_action(cx.listener(Self::handle_tree_confirm_edit))
            .on_action(cx.listener(Self::handle_tree_cancel_edit))
            .child(render_list(
                &self.scroll_handle,
                len,
                is_focused,
                render_context,
            ))
    }
}

// ── 私有渲染辅助函数 ────────────────────────────────────────────────

fn rename_destination(from: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let parent = from
        .parent()
        .ok_or_else(|| anyhow::anyhow!("条目没有父目录"))?;
    entry_destination(parent, name)
}

fn entry_destination(parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(!name.is_empty(), "名称不能为空");
    anyhow::ensure!(name != "." && name != "..", "名称不能是 {name}");
    anyhow::ensure!(
        !name.contains(['/', '\\', '\0']),
        "名称不能包含路径分隔符或空字符"
    );
    Ok(parent.join(name))
}

#[derive(Debug, PartialEq, Eq)]
struct NewEntryDestination {
    path: PathBuf,
    is_dir: bool,
}

fn new_entry_destination(parent: &Path, input: &str) -> anyhow::Result<NewEntryDestination> {
    anyhow::ensure!(!input.trim().is_empty(), "名称不能为空");
    anyhow::ensure!(!input.starts_with('/'), "新条目必须使用相对路径");
    anyhow::ensure!(!input.contains(['\\', '\0']), "名称不能包含反斜杠或空字符");

    let is_dir = input.ends_with('/');
    let relative = input.trim_end_matches('/');
    anyhow::ensure!(!relative.is_empty(), "名称不能为空");
    let mut path = parent.to_path_buf();
    for component in relative.split('/') {
        anyhow::ensure!(!component.trim().is_empty(), "路径不能包含空名称");
        anyhow::ensure!(
            component != "." && component != "..",
            "路径不能包含 {component}"
        );
        path.push(component);
    }

    Ok(NewEntryDestination { path, is_dir })
}

fn translate_path(path: &Path, from: &Path, to: &Path) -> PathBuf {
    path.strip_prefix(from)
        .map_or_else(|_| path.to_path_buf(), |suffix| to.join(suffix))
}

fn render_list(
    scroll_handle: &UniformListScrollHandle,
    len: usize,
    is_focused: bool,
    render_context: ProjectTreeRenderContext,
) -> gpui::UniformList {
    let handle = scroll_handle.clone();

    uniform_list("project-tree-list", len, move |range, _, cx| {
        let state = render_context.state.borrow();
        let rows = &render_context.rows;
        range
            .filter_map(|i| rows.get(i))
            .map(|row| {
                let sel = row.is_new || state.selected.as_ref() == Some(&row.path);
                let marked = !row.is_new && state.active_path.as_ref() == Some(&row.path);
                render_row(row, sel, marked, is_focused, render_context.clone(), cx)
                    .into_any_element()
            })
            .collect()
    })
    .size_full()
    .track_scroll(handle)
}

fn render_row(
    row: &ProjectTreeRow,
    sel: bool,
    marked: bool,
    focused: bool,
    render_context: ProjectTreeRenderContext,
    cx: &mut App,
) -> Div {
    let path = row.path.clone();
    let is_dir = row.is_dir;
    let depth = row.depth;
    let name = row.name.clone();
    let is_editing = render_context
        .edit_state
        .as_ref()
        .is_some_and(|edit_state| edit_state.matches_row(row));
    let has_error = is_editing
        && render_context
            .edit_state
            .as_ref()
            .is_some_and(|edit_state| edit_state.validation_error.is_some());
    let content = if is_editing {
        div()
            .key_context("ProjectTreeEdit")
            .flex_1()
            .overflow_hidden()
            .when(has_error, |element| {
                element
                    .border_1()
                    .border_color(color::current(cx).status_error)
            })
            .child(render_context.entry_name_editor.clone())
    } else {
        // git 状态决定文件名颜色（对齐 Zed 优先级），忽略条目淡显。
        let status_color = row
            .git_status
            .and_then(|status| git_status_color(status, cx));
        let is_ignored = matches!(row.git_status, Some(FileStatus::Ignored));
        div()
            .flex_1()
            .overflow_hidden()
            .truncate()
            .when_some(status_color, |element, status_color| {
                element.text_color(status_color)
            })
            .when(is_ignored && status_color.is_none(), |element| {
                element.text_color(color::current(cx).text_muted)
            })
            .child(name)
    };

    tree::render_row_base(depth, is_dir, row.expanded, content, cx)
        .cursor_pointer()
        .when(marked, |el| el.bg(color::current(cx).element_selected))
        .hover(|style| style.bg(color::current(cx).element_hover))
        .when(sel && focused, |el| el.child(tree::selection_border(cx)))
        .when(!is_editing, |row| {
            row.on_mouse_down(MouseButton::Left, {
                // 焦点句柄与 weak 引用先取出，避免 move 整个 render_context。
                let focus = render_context.focus.clone();
                let weak = render_context.weak.clone();
                move |event, window, cx| {
                    // 单击/双击都把焦点收到项目树（交互规范：单击后焦点留在项目树；
                    // 对齐 Zed 的 cx.listener 路径，直接调用 Entity 方法，不走 action 分派）。
                    window.focus(&focus);
                    if let Some(tree) = weak.upgrade() {
                        tree.update(cx, |tree, cx| {
                            tree.state.borrow_mut().select(&path);
                            match event.click_count {
                                // 单击：目录展开/折叠、文件预览（焦点留在项目树）；
                                // 双击：文件打开并聚焦编辑器；目录不重复，避免"展开→折叠"抵消。
                                1 => tree.activate_selected(false, window, cx),
                                _ if is_dir => {}
                                _ => tree.activate_selected(true, window, cx),
                            }
                        });
                    }
                    cx.stop_propagation();
                }
            })
        })
}

/// git 状态 → 文件名颜色（对齐 Zed `entry_git_aware_label_color` 的优先级）。
///
/// conflict > deleted > modified > added/untracked > ignored（渲染层淡显）。
fn git_status_color(status: FileStatus, cx: &App) -> Option<gpui::Rgba> {
    let colors = color::current(cx);
    match status {
        FileStatus::Unmerged => Some(colors.status_conflict),
        FileStatus::Untracked => Some(colors.status_created),
        FileStatus::Ignored => None,
        FileStatus::Tracked {
            index_status,
            worktree_status,
        } => {
            let deleted = matches!(index_status, StatusCode::Deleted)
                || matches!(worktree_status, StatusCode::Deleted);
            let modified = matches!(index_status, StatusCode::Modified | StatusCode::TypeChanged)
                || matches!(
                    worktree_status,
                    StatusCode::Modified | StatusCode::TypeChanged
                );
            let added = matches!(index_status, StatusCode::Added)
                || matches!(worktree_status, StatusCode::Added);
            if deleted {
                Some(colors.status_deleted)
            } else if modified {
                Some(colors.status_modified)
            } else if added {
                Some(colors.status_created)
            } else {
                None
            }
        }
    }
}

impl Panel for ProjectTree {
    type ToggleAction = ToggleProjectTree;

    fn icon() -> &'static str {
        "icons/panels/project_tree.svg"
    }
    fn label() -> &'static str {
        "项目树"
    }
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

// ── 内部类型 ────────────────────────────────────────────────────────

#[derive(Clone)]
struct ProjectTreeRenderContext {
    state: Rc<RefCell<ProjectTreeState>>,
    rows: Rc<[ProjectTreeRow]>,
    focus: gpui::FocusHandle,
    /// 条目点击直接调用 Entity 方法（对齐 Zed 的 `cx.listener` 路径），
    /// 不依赖 dispatch_action 的焦点链分发。
    weak: WeakEntity<ProjectTree>,
    edit_state: Option<EditState>,
    entry_name_editor: Entity<Editor>,
}

#[derive(Clone, Debug)]
struct EditState {
    operation: EditOperation,
    validation_error: Option<String>,
}

impl EditState {
    fn matches_row(&self, row: &ProjectTreeRow) -> bool {
        match &self.operation {
            EditOperation::Rename { source, is_dir } => {
                !row.is_new && row.path == *source && row.is_dir == *is_dir
            }
            EditOperation::Create { parent } => row.is_new && row.path == *parent,
        }
    }
}

#[derive(Clone, Debug)]
enum EditOperation {
    Rename { source: PathBuf, is_dir: bool },
    Create { parent: PathBuf },
}

#[derive(Debug, Clone)]
struct ProjectTreeRow {
    path: PathBuf,
    name: String,
    depth: usize,
    is_dir: bool,
    expanded: bool,
    is_new: bool,
    /// git 状态（决定文件名颜色与忽略淡显；None 表示无状态）。
    git_status: Option<FileStatus>,
}

/// 项目树的过滤规则：扫描排除（glob 名单）。
///
/// file_scan_exclusions 命中的条目根本不在树中加载；
/// 忽略（gitignore/info/exclude）由 git 状态统一判定（`FileStatus::Ignored`）。
struct TreeFilter {
    /// 用户配置的扫描排除 glob。
    exclusions: GlobSet,
}

impl TreeFilter {
    fn new(exclusions: &[String]) -> Self {
        let mut builder = GlobSetBuilder::new();
        for glob in exclusions {
            if let Ok(glob) = Glob::new(glob) {
                builder.add(glob);
            }
        }
        Self {
            exclusions: builder.build().unwrap_or_default(),
        }
    }

    /// 路径的任一祖先命中排除名单即排除。
    fn is_excluded(&self, rel_path: &Path) -> bool {
        rel_path
            .ancestors()
            .any(|ancestor| self.exclusions.is_match(ancestor))
    }
}

struct ProjectTreeState {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    active_path: Option<PathBuf>,
    rows: Vec<ProjectTreeRow>,
    /// 扫描排除 glob（由 ProjectTree 注入），根目录变化时用于重建过滤。
    exclusions: Vec<String>,
    filter: TreeFilter,
    /// 路径 → git 状态（由 ProjectTree 经 project 查询注入）。
    git_statuses: HashMap<PathBuf, FileStatus>,
}

impl ProjectTreeState {
    fn new(root: PathBuf) -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        let filter = TreeFilter::new(&[]);
        let mut state = Self {
            root,
            expanded,
            selected: None,
            active_path: None,
            rows: Vec::new(),
            exclusions: Vec::new(),
            filter,
            git_statuses: HashMap::new(),
        };
        state.refresh_rows();
        state
    }

    /// 注入扫描排除规则并重建过滤（由 ProjectTree 在创建或设置变化时调用）。
    fn set_filter(&mut self, exclusions: &[String]) {
        self.exclusions = exclusions.to_vec();
        self.filter = TreeFilter::new(exclusions);
    }

    /// 更换根目录，重置展开和选中状态。
    fn set_root(&mut self, root: PathBuf) {
        self.root = root;
        self.expanded.clear();
        self.expanded.insert(self.root.clone());
        self.selected = None;
        self.active_path = None;
        self.git_statuses.clear();
        self.filter = TreeFilter::new(&self.exclusions);
        self.refresh_rows();
    }

    /// 替换 git 状态表并逐行更新（git 事件驱动，不重扫目录）。
    fn update_git_statuses(&mut self, statuses: HashMap<PathBuf, FileStatus>) {
        self.git_statuses = statuses;
        for row in &mut self.rows {
            row.git_status = self.git_statuses.get(&row.path).copied();
        }
    }

    fn visible_rows(&self) -> &[ProjectTreeRow] {
        &self.rows
    }

    fn refresh_rows(&mut self) {
        let mut rows = Vec::new();

        // 根目录本身作为 depth 0 行
        let root_name = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.root.to_string_lossy().to_string());
        let root_expanded = self.expanded.contains(&self.root);
        rows.push(ProjectTreeRow {
            path: self.root.clone(),
            name: root_name,
            depth: 0,
            is_dir: true,
            expanded: root_expanded,
            is_new: false,
            git_status: None,
        });

        if root_expanded {
            let root = self.root.clone();
            self.collect_children(&root, 1, &mut rows);
        }

        self.rows = rows;
        if self
            .selected
            .as_ref()
            .is_some_and(|selected| !self.rows.iter().any(|row| &row.path == selected))
        {
            self.selected = None;
        }
    }

    /// 确保有选中行：无选中时选中第一行。
    fn ensure_selected(&mut self) {
        if self.selected.is_some() {
            return;
        }
        if let Some(first) = self.rows.first() {
            self.selected = Some(first.path.clone());
        }
    }

    /// 递归收集目录子项；忽略（gitignored 目录不展开）由 git 状态统一判定。
    fn collect_children(&mut self, dir: &Path, depth: usize, rows: &mut Vec<ProjectTreeRow>) {
        let mut entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
            Err(_) => return,
        };
        entries.sort_by(|a, b| {
            let a_dir = a.is_dir();
            let b_dir = b.is_dir();
            if a_dir != b_dir {
                b_dir.cmp(&a_dir)
            } else {
                a.file_name().cmp(&b.file_name())
            }
        });
        for entry in entries {
            let name = match entry.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };
            let is_dir = entry.is_dir();
            // 扫描排除名单命中的条目根本不加载。
            let Ok(rel) = entry.strip_prefix(&self.root) else {
                continue;
            };
            if self.filter.is_excluded(rel) {
                continue;
            }
            let git_status = self.git_statuses.get(&entry).copied();
            let is_expanded = self.expanded.contains(&entry);
            rows.push(ProjectTreeRow {
                path: entry.clone(),
                name,
                depth,
                is_dir,
                expanded: is_expanded,
                is_new: false,
                git_status,
            });
            // 被忽略的目录不展开内容，避免 node_modules 这类目录撑爆行模型。
            if is_dir && is_expanded && !matches!(git_status, Some(FileStatus::Ignored)) {
                self.collect_children(&entry, depth + 1, rows);
            }
        }
    }

    fn toggle_expand(&mut self, path: &Path) {
        if self.expanded.contains(path) {
            self.expanded.remove(path);
        } else {
            self.expanded.insert(path.to_path_buf());
        }
        self.refresh_rows();
    }

    fn select(&mut self, path: &Path) {
        self.selected = Some(path.to_path_buf());
    }

    /// 对齐 Zed 的 reveal_entry：展开祖先目录，同时更新 selection 与 marked 状态。
    fn reveal_active_path(&mut self, path: Option<&Path>) -> Option<usize> {
        let Some(path) = path.filter(|path| path.starts_with(&self.root)) else {
            self.active_path = None;
            return None;
        };

        let mut ancestor = path.parent();
        while let Some(directory) = ancestor.filter(|directory| directory.starts_with(&self.root)) {
            self.expanded.insert(directory.to_path_buf());
            if directory == self.root {
                break;
            }
            ancestor = directory.parent();
        }

        self.active_path = Some(path.to_path_buf());
        self.selected = Some(path.to_path_buf());
        self.refresh_rows();
        self.rows.iter().position(|row| row.path == path)
    }

    fn selected_idx(&self, rows: &[ProjectTreeRow]) -> Option<usize> {
        self.selected
            .as_ref()
            .and_then(|sel| rows.iter().position(|r| r.path == *sel))
    }

    fn select_up(&mut self, rows: &[ProjectTreeRow]) {
        let idx = self.selected_idx(rows).unwrap_or(0);
        if idx > 0 {
            self.selected = Some(rows[idx - 1].path.clone());
        }
    }

    fn select_down(&mut self, rows: &[ProjectTreeRow]) {
        let idx = self.selected_idx(rows).unwrap_or(0);
        if idx + 1 < rows.len() {
            self.selected = Some(rows[idx + 1].path.clone());
        } else if self.selected.is_none() && !rows.is_empty() {
            self.selected = Some(rows[0].path.clone());
        }
    }

    fn apply_rename(&mut self, from: &Path, to: &Path) {
        self.root = translate_path(&self.root, from, to);
        self.expanded = self
            .expanded
            .drain()
            .map(|path| translate_path(&path, from, to))
            .collect();
        self.selected = self
            .selected
            .take()
            .map(|path| translate_path(&path, from, to));
        self.active_path = self
            .active_path
            .take()
            .map(|path| translate_path(&path, from, to));
        self.refresh_rows();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui::{AppContext, KeyBinding, Render, TestAppContext, point, px};

    use super::*;

    struct TestView;

    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[test]
    fn file_scan_exclusions_hide_entries_and_their_children() {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let target = directory.path().join("target");
        std::fs::create_dir_all(target.join("debug")).expect("应创建排除目录");
        std::fs::write(target.join("debug").join("app"), "binary").expect("应创建排除文件");
        let visible = directory.path().join("main.rs");
        std::fs::write(&visible, "fn main() {}").expect("应创建可见文件");

        let mut state = ProjectTreeState::new(directory.path().to_path_buf());
        state.set_filter(&["**/target".to_string()]);
        state.expanded.insert(target.clone());
        state.refresh_rows();

        let rows = state.visible_rows().to_vec();
        assert!(
            !rows
                .iter()
                .any(|row| row.path == target || row.path.starts_with(&target)),
            "排除名单命中的目录及其子项都不应出现"
        );
        assert!(rows.iter().any(|row| row.path == visible));
    }

    #[test]
    fn ignored_directories_do_not_expand_and_files_are_marked() {
        // 忽略信息来自 git 状态（FileStatus::Ignored），由外部注入。
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let node_modules = directory.path().join("node_modules");
        std::fs::create_dir_all(node_modules.join("pkg")).expect("应创建被忽略目录");
        std::fs::write(node_modules.join("pkg").join("index.js"), "// ignored")
            .expect("应创建被忽略文件");
        std::fs::write(directory.path().join("app.log"), "log").expect("应创建日志文件");
        let visible = directory.path().join("main.js");
        std::fs::write(&visible, "console.log(1)").expect("应创建可见文件");

        let mut state = ProjectTreeState::new(directory.path().to_path_buf());
        state.update_git_statuses(HashMap::from([
            (node_modules.clone(), FileStatus::Ignored),
            (directory.path().join("app.log"), FileStatus::Ignored),
        ]));
        state.expanded.insert(node_modules.clone());
        state.refresh_rows();

        let rows = state.visible_rows().to_vec();
        let nm = rows
            .iter()
            .find(|row| row.path == node_modules)
            .expect("node_modules 行应存在");
        assert!(matches!(nm.git_status, Some(FileStatus::Ignored)));
        assert!(
            !rows
                .iter()
                .any(|row| row.path.starts_with(&node_modules.join("pkg"))),
            "被忽略目录不应展开内容"
        );
        assert!(
            rows.iter()
                .find(|row| row.path == directory.path().join("app.log"))
                .is_some_and(|row| matches!(row.git_status, Some(FileStatus::Ignored))),
            "*.log 文件应被标记为忽略"
        );
        assert!(
            !rows
                .iter()
                .find(|row| row.path == visible)
                .expect("可见文件行应存在")
                .git_status
                .is_some()
        );
    }

    #[gpui::test]
    fn git_status_color_follows_zed_priority(cx: &mut TestAppContext) {
        cx.read(|cx| {
            // palette 未初始化时默认 one_dark，语义色可直接取。
            let colors = color::current(cx);
            let color = |status| git_status_color(status, cx);
            // 特殊态。
            assert_eq!(color(FileStatus::Untracked), Some(colors.status_created));
            assert_eq!(color(FileStatus::Unmerged), Some(colors.status_conflict));
            assert_eq!(color(FileStatus::Ignored), None);
            // 已跟踪：deleted > modified > added 优先级。
            let tracked = |index, worktree| FileStatus::Tracked {
                index_status: index,
                worktree_status: worktree,
            };
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Modified)),
                Some(colors.status_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Modified, StatusCode::Unmodified)),
                Some(colors.status_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::TypeChanged)),
                Some(colors.status_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Added)),
                Some(colors.status_created)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Deleted)),
                Some(colors.status_deleted)
            );
            // 部分暂存：modified 优先于 added。
            assert_eq!(
                color(tracked(StatusCode::Added, StatusCode::Modified)),
                Some(colors.status_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Unmodified)),
                None
            );
        });
    }

    #[test]
    fn nested_gitignore_applies_within_its_directory() {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let sub = directory.path().join("sub");
        let nested = sub.join("secret.txt");
        std::fs::create_dir_all(&sub).expect("应创建子目录");
        std::fs::write(sub.join(".gitignore"), "secret.txt\n").expect("应写嵌套 .gitignore");
        std::fs::write(&nested, "secret").expect("应创建被忽略文件");
        let visible = sub.join("visible.txt");
        std::fs::write(&visible, "visible").expect("应创建可见文件");
        std::fs::write(directory.path().join(".gitignore"), "*.log\n").expect("应写根 .gitignore");

        let mut state = ProjectTreeState::new(directory.path().to_path_buf());
        state.update_git_statuses(HashMap::from([
            (nested.clone(), FileStatus::Ignored),
            (visible.clone(), FileStatus::Untracked),
        ]));
        state.expanded.insert(sub.clone());
        state.refresh_rows();

        let rows = state.visible_rows().to_vec();
        assert!(
            rows.iter()
                .find(|row| row.path == nested)
                .expect("secret.txt 行应存在")
                .git_status
                .is_some_and(|status| matches!(status, FileStatus::Ignored)),
            "嵌套目录的忽略规则应生效"
        );
        assert!(
            !rows
                .iter()
                .find(|row| row.path == visible)
                .expect("visible.txt 行应存在")
                .git_status
                .is_some_and(|status| matches!(status, FileStatus::Ignored))
        );
    }

    #[test]
    fn visible_rows_use_cached_filesystem_model_until_explicit_refresh() {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let file = directory.path().join("cached.txt");
        std::fs::write(&file, "content").expect("应创建测试文件");
        let mut state = ProjectTreeState::new(directory.path().to_path_buf());

        assert!(state.visible_rows().iter().any(|row| row.path == file));
        std::fs::remove_file(&file).expect("应删除测试文件");
        assert!(
            state.visible_rows().iter().any(|row| row.path == file),
            "渲染读取缓存时不应自行扫描文件系统"
        );

        state.refresh_rows();
        assert!(!state.visible_rows().iter().any(|row| row.path == file));
    }

    #[test]
    fn revealing_active_file_expands_ancestors_and_keeps_mark_separate_from_selection() {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let nested = directory.path().join("src").join("feature");
        std::fs::create_dir_all(&nested).expect("应创建嵌套目录");
        let file = nested.join("mod.rs");
        std::fs::write(&file, "content").expect("应创建测试文件");
        let mut state = ProjectTreeState::new(directory.path().to_path_buf());

        let index = state
            .reveal_active_path(Some(&file))
            .expect("活动文件应出现在可见行中");

        assert_eq!(state.active_path.as_deref(), Some(file.as_path()));
        assert_eq!(state.selected.as_deref(), Some(file.as_path()));
        assert_eq!(state.visible_rows()[index].path, file);
        assert!(state.expanded.contains(&directory.path().join("src")));
        assert!(state.expanded.contains(&nested));

        let rows = state.visible_rows().to_vec();
        state.select_up(&rows);
        assert_ne!(state.selected.as_deref(), Some(file.as_path()));
        assert_eq!(
            state.active_path.as_deref(),
            Some(file.as_path()),
            "键盘游标移动不应改变活动文件标记"
        );
    }

    #[test]
    fn revealing_path_outside_project_clears_active_mark() {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let file = directory.path().join("active.txt");
        std::fs::write(&file, "content").expect("应创建测试文件");
        let mut state = ProjectTreeState::new(directory.path().to_path_buf());
        state.reveal_active_path(Some(&file));

        assert!(
            state
                .reveal_active_path(Some(Path::new("/outside/project.txt")))
                .is_none()
        );
        assert!(state.active_path.is_none());
    }

    #[test]
    fn rename_destination_accepts_one_name_and_rejects_paths() {
        let source = Path::new("/project/src/main.rs");

        assert_eq!(
            rename_destination(source, "lib.rs").unwrap(),
            Path::new("/project/src/lib.rs")
        );
        for invalid in ["", ".", "..", "nested/lib.rs", "nested\\lib.rs"] {
            assert!(rename_destination(source, invalid).is_err());
        }
    }

    #[test]
    fn new_entry_destination_uses_a_trailing_slash_for_nested_directories() {
        let parent = Path::new("/project");

        assert_eq!(
            new_entry_destination(parent, "src/components/button.rs").unwrap(),
            NewEntryDestination {
                path: PathBuf::from("/project/src/components/button.rs"),
                is_dir: false,
            }
        );
        assert_eq!(
            new_entry_destination(parent, "assets/icons/").unwrap(),
            NewEntryDestination {
                path: PathBuf::from("/project/assets/icons"),
                is_dir: true,
            }
        );
        for invalid in [
            "",
            "/absolute",
            "src//main.rs",
            "../outside",
            "src\\main.rs",
        ] {
            assert!(new_entry_destination(parent, invalid).is_err());
        }
    }

    #[test]
    fn applying_directory_rename_migrates_tree_paths() {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let old_directory = directory.path().join("old");
        let old_file = old_directory.join("mod.rs");
        std::fs::create_dir(&old_directory).expect("应创建待重命名目录");
        std::fs::write(&old_file, "content").expect("应创建测试文件");
        let mut state = ProjectTreeState::new(directory.path().to_path_buf());
        state.expanded.insert(old_directory.clone());
        state.selected = Some(old_file.clone());
        state.active_path = Some(old_file.clone());
        state.refresh_rows();

        let new_directory = directory.path().join("new");
        std::fs::rename(&old_directory, &new_directory).expect("应重命名测试目录");
        state.apply_rename(&old_directory, &new_directory);

        let new_file = new_directory.join("mod.rs");
        assert!(state.expanded.contains(&new_directory));
        assert_eq!(state.selected.as_deref(), Some(new_file.as_path()));
        assert_eq!(state.active_path.as_deref(), Some(new_file.as_path()));
        assert!(state.visible_rows().iter().any(|row| row.path == new_file));
    }

    #[gpui::test]
    fn space_edits_the_name_instead_of_activating_the_row_while_renaming(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let file = directory.path().join("old.txt");
        std::fs::write(&file, "content").expect("应创建测试文件");
        let project_root = directory.path().to_path_buf();
        let selected_file = file.clone();
        let open_count = Rc::new(Cell::new(0));
        let callback_count = Rc::clone(&open_count);

        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (tree, cx) = cx.add_window_view(move |_, cx| {
            cx.bind_keys([
                KeyBinding::new("enter", TreeRename, Some("ProjectTree && not_editing")),
                KeyBinding::new("space", TreeActivate, Some("ProjectTree && not_editing")),
            ]);
            let mut tree = ProjectTree::new(project_root, project.clone(), cx);
            tree.set_on_open_file(Rc::new(move |_, _, _, _| {
                callback_count.set(callback_count.get() + 1);
            }));
            tree.state.borrow_mut().select(&selected_file);
            tree
        });
        cx.update(|window, cx| window.focus(&tree.read(cx).focus));

        cx.simulate_keystrokes("enter");
        let entry_name_editor = cx.read_entity(&tree, |tree, _| {
            assert!(matches!(
                tree.edit_state.as_ref().map(|state| &state.operation),
                Some(EditOperation::Rename { source, .. }) if source == &file
            ));
            tree.entry_name_editor.clone()
        });
        cx.simulate_keystrokes("space");

        assert_eq!(open_count.get(), 0);
        cx.read_entity(&tree, |tree, _| assert!(tree.edit_state.is_some()));
        cx.read_entity(&entry_name_editor, |editor, cx| {
            assert_eq!(editor.text(cx), " .txt");
        });
    }

    #[gpui::test]
    fn mouse_click_opens_file_even_when_tree_not_focused(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let file = directory.path().join("a.txt");
        std::fs::write(&file, "hello").expect("应创建测试文件");
        let project_root = directory.path().to_path_buf();

        // 记录每次打开回调的 focus_opened_item：单击应为 false（预览），双击应为 true（激活）。
        let open_count = Rc::new(Cell::new(0));
        let last_focus_opened = Rc::new(Cell::new(true));
        let callback_count = Rc::clone(&open_count);
        let callback_focus = Rc::clone(&last_focus_opened);

        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (_tree, cx) = cx.add_window_view(move |_, cx| {
            let mut tree = ProjectTree::new(project_root, project.clone(), cx);
            tree.set_on_open_file(Rc::new(move |_, focus_opened_item, _, _| {
                callback_count.set(callback_count.get() + 1);
                callback_focus.set(focus_opened_item);
            }));
            // 展开根目录使文件行可见；不聚焦项目树（模拟焦点在别处）。
            tree.state.borrow_mut().expanded.insert(tree.root.clone());
            tree.state.borrow_mut().refresh_rows();
            tree
        });
        cx.run_until_parked();

        // 单击第二行（a.txt，depth 1）：行高为 ui_line()。
        let row_height = zcv_theme::typography::ui_line();
        cx.simulate_click(
            point(px(10.), px(f32::from(row_height) + 1.)),
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();

        assert_eq!(
            open_count.get(),
            1,
            "焦点不在项目树时单击文件也应打开（action 沿焦点链分发，点击需先聚焦项目树）"
        );
        assert!(
            !last_focus_opened.get(),
            "单击文件应为预览：打开文件但焦点留在项目树（focus_opened_item=false）"
        );
    }

    #[gpui::test]
    fn rename_actions_edit_and_confirm_the_selected_row(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let old_path = directory.path().join("old.txt");
        let new_path = directory.path().join("new.txt");
        std::fs::write(&old_path, "content").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
        let tree =
            cx.new(|cx| ProjectTree::new(directory.path().to_path_buf(), project.clone(), cx));
        tree.update(cx, |tree, _| {
            tree.set_on_rename(Rc::new(|from, to, _| {
                std::fs::rename(from, to)?;
                Ok(())
            }));
            tree.state.borrow_mut().select(&old_path);
        });

        cx.add_window_view(|window, cx| {
            tree.update(cx, |tree, cx| {
                tree.handle_tree_rename(&TreeRename, window, cx);
                tree.entry_name_editor
                    .update(cx, |editor, cx| editor.set_text("new.txt", cx));
                tree.handle_tree_confirm_edit(&TreeConfirmEdit, window, cx);
            });
            TestView
        });

        assert!(!old_path.exists());
        assert!(new_path.exists());
        cx.read_entity(&tree, |tree, _| {
            assert!(tree.edit_state.is_none());
            assert_eq!(
                tree.state.borrow().selected.as_deref(),
                Some(new_path.as_path())
            );
        });
    }

    #[gpui::test]
    fn one_create_action_infers_nested_files_and_directories_from_the_path(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let file = directory.path().join("src/components/button.rs");
        let folder = directory.path().join("assets/icons");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
        let tree =
            cx.new(|cx| ProjectTree::new(directory.path().to_path_buf(), project.clone(), cx));
        tree.update(cx, |tree, _| {
            tree.set_on_create(Rc::new(|path, is_dir, _| {
                std::fs::create_dir_all(path.parent().unwrap())?;
                if is_dir {
                    std::fs::create_dir(path)?;
                } else {
                    std::fs::File::create(path)?;
                }
                Ok(())
            }));
        });

        cx.add_window_view(|window, cx| {
            tree.update(cx, |tree, cx| {
                tree.state.borrow_mut().ensure_selected();
                tree.handle_tree_new_entry(&TreeNewEntry, window, cx);
                tree.entry_name_editor.update(cx, |editor, cx| {
                    editor.set_text("src/components/button.rs", cx)
                });
                assert!(
                    tree.display_rows(cx)
                        .iter()
                        .any(|row| row.is_new && !row.is_dir)
                );
                tree.handle_tree_confirm_edit(&TreeConfirmEdit, window, cx);

                tree.state.borrow_mut().select(directory.path());
                tree.handle_tree_new_entry(&TreeNewEntry, window, cx);
                tree.entry_name_editor
                    .update(cx, |editor, cx| editor.set_text("assets/icons/", cx));
                assert!(
                    tree.display_rows(cx)
                        .iter()
                        .any(|row| row.is_new && row.is_dir)
                );
                tree.handle_tree_confirm_edit(&TreeConfirmEdit, window, cx);
            });
            TestView
        });

        assert!(file.is_file());
        assert!(folder.is_dir());
        cx.read_entity(&tree, |tree, _| {
            assert!(tree.edit_state.is_none());
            assert_eq!(
                tree.state.borrow().selected.as_deref(),
                Some(folder.as_path())
            );
        });
    }

    #[gpui::test]
    fn trash_action_moves_the_selected_row_to_trash_and_selects_the_next_row(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let trashed_file = directory.path().join("trash-me.txt");
        let kept_file = directory.path().join("keep.txt");
        std::fs::write(&trashed_file, "content").expect("应创建测试文件");
        std::fs::write(&kept_file, "content").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
        let tree =
            cx.new(|cx| ProjectTree::new(directory.path().to_path_buf(), project.clone(), cx));
        let trashed = Rc::new(RefCell::new(None));
        let trashed_path = Rc::clone(&trashed);
        tree.update(cx, |tree, _| {
            tree.set_on_trash(Rc::new(move |path, _, _| {
                std::fs::remove_file(&path)?;
                *trashed_path.borrow_mut() = Some(path);
                Ok(())
            }));
            tree.state.borrow_mut().select(&trashed_file);
        });

        cx.add_window_view(|window, cx| {
            tree.update(cx, |tree, cx| {
                tree.handle_tree_trash(&TreeTrash, window, cx);
            });
            TestView
        });

        assert_eq!(trashed.borrow().as_deref(), Some(trashed_file.as_path()));
        assert!(!trashed_file.exists());
        assert!(kept_file.exists());
        cx.read_entity(&tree, |tree, _| {
            assert_eq!(
                tree.state.borrow().selected.as_deref(),
                Some(kept_file.as_path()),
                "删除后应选中原位置的下一个条目"
            );
        });
    }

    #[gpui::test]
    fn trash_action_ignores_the_root_row(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
        let tree =
            cx.new(|cx| ProjectTree::new(directory.path().to_path_buf(), project.clone(), cx));
        let called = Rc::new(Cell::new(false));
        let callback_called = Rc::clone(&called);
        tree.update(cx, |tree, _| {
            tree.set_on_trash(Rc::new(move |_, _, _| {
                callback_called.set(true);
                Ok(())
            }));
            tree.state.borrow_mut().select(directory.path());
        });

        cx.add_window_view(|window, cx| {
            tree.update(cx, |tree, cx| {
                tree.handle_tree_trash(&TreeTrash, window, cx);
            });
            TestView
        });

        assert!(!called.get(), "根目录行不应触发删除");
        assert!(directory.path().exists());
    }

    #[gpui::test]
    fn trash_action_selects_the_last_row_after_deleting_the_final_entry(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let only_file = directory.path().join("only.txt");
        std::fs::write(&only_file, "content").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
        let tree =
            cx.new(|cx| ProjectTree::new(directory.path().to_path_buf(), project.clone(), cx));
        tree.update(cx, |tree, _| {
            tree.set_on_trash(Rc::new(|path, _, _| {
                std::fs::remove_file(path)?;
                Ok(())
            }));
            tree.state.borrow_mut().select(&only_file);
        });

        cx.add_window_view(|window, cx| {
            tree.update(cx, |tree, cx| {
                tree.handle_tree_trash(&TreeTrash, window, cx);
            });
            TestView
        });

        cx.read_entity(&tree, |tree, _| {
            assert_eq!(
                tree.state.borrow().selected.as_deref(),
                Some(directory.path()),
                "删除最后一项后应选中新的最后一行（根目录）"
            );
        });
    }

    #[gpui::test]
    fn git_status_events_update_row_colors(cx: &mut TestAppContext) {
        let (root, _temp) = test_git_repo();
        let project = cx.new(|cx| Project::new(root.clone(), cx));
        let tree = cx.new(|cx| ProjectTree::new(root.clone(), project.clone(), cx));
        cx.run_until_parked();

        // 修改文件 → git 增量刷新 → StatusesChanged 事件 → 行颜色更新。
        let file = root.join("tracked.txt");
        std::fs::write(&file, "已修改\n").expect("应修改文件");
        project.update(cx, |project, cx| {
            project.git_store().update(cx, |store, cx| {
                store.refresh_statuses_for_paths(&[file.clone()], cx);
            });
        });
        cx.run_until_parked();

        let status = cx.read_entity(&tree, |tree, _| {
            tree.state
                .borrow()
                .rows
                .iter()
                .find(|row| row.path == file)
                .and_then(|row| row.git_status)
        });
        assert!(
            status.is_some_and(|status| status.is_modified()),
            "行应携带 modified 状态"
        );
    }

    #[gpui::test]
    fn git_status_events_color_directories_with_changed_children(cx: &mut TestAppContext) {
        let (root, _temp) = test_git_repo();
        // 建子目录并提交一个文件。
        let src = root.join("src");
        std::fs::create_dir_all(&src).expect("应创建目录");
        let src_file = src.join("main.rs");
        std::fs::write(&src_file, "fn main() {}\n").expect("应创建文件");
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .expect("应执行成功");
            assert!(output.status.success(), "git {:?} 失败", args);
        };
        run(&["add", "src/main.rs"]);
        run(&["commit", "-q", "-m", "add src"]);

        let project = cx.new(|cx| Project::new(root.clone(), cx));
        let tree = cx.new(|cx| ProjectTree::new(root.clone(), project.clone(), cx));
        cx.run_until_parked();

        // 修改目录内文件 → 增量刷新 → 目录行聚合为 modified。
        std::fs::write(&src_file, "fn main() { println!(); }\n").expect("应修改文件");
        project.update(cx, |project, cx| {
            project.git_store().update(cx, |store, cx| {
                store.refresh_statuses_for_paths(&[src_file.clone()], cx);
            });
        });
        cx.run_until_parked();

        let status = cx.read_entity(&tree, |tree, _| {
            tree.state
                .borrow()
                .rows
                .iter()
                .find(|row| row.path == src)
                .and_then(|row| row.git_status)
        });
        assert!(
            status.is_some_and(|status| status.is_modified()),
            "目录行应聚合子项状态"
        );
    }

    #[gpui::test]
    fn expanding_directory_fills_git_status_for_new_rows(cx: &mut TestAppContext) {
        // 回归：展开目录产生的新行此前未查询过，git 状态缓存需在展开时补齐。
        let (root, _temp) = test_git_repo();
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).expect("应创建目录");
        let sub_file = sub.join("untracked.txt");
        std::fs::write(&sub_file, "x\n").expect("应创建文件");

        let project = cx.new(|cx| Project::new(root.clone(), cx));
        let tree = cx.new(|cx| ProjectTree::new(root.clone(), project.clone(), cx));
        cx.run_until_parked();

        // 展开 sub 目录（模拟 handle_tree_expand 的行重建路径）。
        tree.update(cx, |tree, _| {
            tree.state.borrow_mut().expanded.insert(sub.clone());
            tree.state.borrow_mut().refresh_rows();
        });
        tree.update(cx, |tree, cx| tree.refresh_git_statuses(cx));
        cx.run_until_parked();

        let status = cx.read_entity(&tree, |tree, _| {
            tree.state
                .borrow()
                .rows
                .iter()
                .find(|row| row.path == sub_file)
                .and_then(|row| row.git_status)
        });
        assert!(
            status.is_some_and(|status| status.is_untracked()),
            "展开后新出现的文件行应补齐 git 状态"
        );
    }

    #[gpui::test]
    fn activating_directory_fills_git_status_for_new_rows(cx: &mut TestAppContext) {
        // 回归：鼠标点击/键盘激活目录统一走 TreeActivate handler（toggle_expand），
        // 展开后的新行 git 状态需在激活时补齐。
        let (root, _temp) = test_git_repo();
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).expect("应创建目录");
        let sub_file = sub.join("untracked.txt");
        std::fs::write(&sub_file, "x\n").expect("应创建文件");

        let project = cx.new(|cx| Project::new(root.clone(), cx));
        let tree = cx.new(|cx| ProjectTree::new(root.clone(), project.clone(), cx));
        cx.run_until_parked();

        // 模拟鼠标点击：选中行后激活（与键盘 enter 同一 handler）。
        cx.add_window_view(|window, cx| {
            tree.update(cx, |tree, _| {
                tree.state.borrow_mut().select(&sub);
            });
            tree.update(cx, |tree, cx| {
                tree.handle_tree_activate(&TreeActivate, window, cx);
            });
            TestView
        });

        let status = cx.read_entity(&tree, |tree, _| {
            tree.state
                .borrow()
                .rows
                .iter()
                .find(|row| row.path == sub_file)
                .and_then(|row| row.git_status)
        });
        assert!(
            status.is_some_and(|status| status.is_untracked()),
            "激活展开后新出现的文件行应补齐 git 状态"
        );
    }

    /// 创建带一个初始提交的临时 git 仓库，返回 (仓库根, 目录句柄)。
    fn test_git_repo() -> (PathBuf, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let root = temp_dir.path().to_path_buf();
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .expect("应执行成功");
            assert!(
                output.status.success(),
                "git {:?} 失败：{}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "-q", "-b", "master"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test User"]);
        std::fs::write(root.join("tracked.txt"), "第一行\n第二行\n").expect("应写入初始文件");
        run(&["add", "tracked.txt"]);
        run(&["commit", "-q", "-m", "initial"]);
        (root, temp_dir)
    }
}
