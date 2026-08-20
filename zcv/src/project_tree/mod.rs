//! ProjectTreePanel —— 项目文件树 Entity 组件。
//!
//! 持有 `Rc<RefCell<ProjectTreeState>>` 管理展开/选中状态和缓存行模型。
//! 目录遍历、排除规则与 git 状态合并由 `Project`（worktree 快照层）产出，渲染与键盘导航只消费行模型缓存。

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    App, Context, Div, Entity, KeyContext, MouseButton, ScrollStrategy, UniformListScrollHandle,
    WeakEntity, Window, div, prelude::*, uniform_list,
};
use zcv_actions::{
    TreeActivate, TreeCancelEdit, TreeCollapse, TreeConfirmEdit, TreeExpand, TreeNewEntry,
    TreeRename, TreeSelectNext, TreeSelectPrev, TreeTrash,
};
use zcv_editor::Editor;
use zcv_git::FileStatus;
use zcv_project::{Project, new_entry_destination, rename_destination, translate_path};
use zcv_theme::color;
use zcv_ui::Scrollbar;
use zcv_ui::tree::{self, TreeRow, TreeState};
use zcv_workspace::Panel;

use crate::git_status::git_status_color;
use crate::workspace::{OnCreate, OnOpenFile, OnRename, OnTrash};
use zcv_settings::SettingsStore;

// ── Entity ──────────────────────────────────────────────────────────

pub(crate) struct ProjectTreePanel {
    pub focus: gpui::FocusHandle,
    /// 当前项目根目录路径；无 worktree 的空工作区为 None（面板显示空态）。
    root: Option<PathBuf>,
    /// 行模型与 git 状态查询（worktree 快照层由 Project 持有）。
    project: Entity<Project>,
    state: Rc<RefCell<TreeState<PathBuf, ProjectTreeRow>>>,
    /// 当前活动文件（编辑器焦点所在文件），与选中行相互独立。
    active_path: Option<PathBuf>,
    scroll_handle: UniformListScrollHandle,
    scrollbar: Scrollbar<UniformListScrollHandle>,
    entry_name_editor: Entity<Editor>,
    edit_state: Option<EditState>,
    on_open_file: Option<OnOpenFile>,
    on_rename: Option<OnRename>,
    on_create: Option<OnCreate>,
    on_trash: Option<OnTrash>,
}

impl ProjectTreePanel {
    pub(crate) fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let entry_name_editor = cx.new(Editor::single_line);
        cx.observe(&entry_name_editor, |_, _, cx| cx.notify())
            .detach();
        let exclusions = SettingsStore::file_scan_exclusions(cx);
        project.update(cx, |project, _| project.set_exclusions(&exclusions));
        // 项目根从 Project 派生：无 worktree 时为空态，面板同样注册（对齐 Zed 无条件装配）。
        let mut state = TreeState::new(|row: &ProjectTreeRow| Some(row.path.clone()));
        if let Some(root) = project.read(cx).root() {
            state.expanded.insert(root.to_path_buf());
        }
        // git 状态变化（含忽略集变化）时刷新行颜色，不重扫目录。
        let git_store = project.read(cx).git_store();
        cx.subscribe(&git_store, |tree, _, _event, cx| {
            tree.refresh_git_statuses(cx);
        })
        .detach();
        let scroll_handle = UniformListScrollHandle::default();
        let scrollbar = Scrollbar::vertical(scroll_handle.clone());
        let mut this = Self {
            focus,
            root: project.read(cx).root().map(PathBuf::from),
            project,
            state: Rc::new(RefCell::new(state)),
            active_path: None,
            scroll_handle,
            scrollbar,
            entry_name_editor,
            edit_state: None,
            on_open_file: None,
            on_rename: None,
            on_create: None,
            on_trash: None,
        };
        this.rebuild_rows(cx);
        this
    }

    /// 重建可见行：根行 + 按展开状态递归收集子项。
    ///
    /// git 状态由 `Project::children` 查询填充；
    /// 展开、深度与可见行是本视图的状态，展开/折叠后调用本方法重建。
    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            // 无 worktree 的空态：行模型为空。
            self.state.borrow_mut().replace_rows(Vec::new());
            return;
        };
        let expanded = self.state.borrow().expanded.clone();
        let mut rows = Vec::new();
        let root_name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.to_string_lossy().to_string());
        let root_expanded = expanded.contains(&root);
        rows.push(ProjectTreeRow {
            path: root.clone(),
            name: root_name,
            depth: 0,
            is_dir: true,
            expanded: root_expanded,
            is_new: false,
            git_status: None,
        });
        if root_expanded {
            self.collect_children(&root, 1, &expanded, &mut rows, cx);
        }
        self.state.borrow_mut().replace_rows(rows);
    }

    /// 递归收集目录子项；被忽略（gitignored）的目录不展开内容，避免 node_modules 这类目录撑爆行模型。
    fn collect_children(
        &self,
        dir: &Path,
        depth: usize,
        expanded: &HashSet<PathBuf>,
        rows: &mut Vec<ProjectTreeRow>,
        cx: &App,
    ) {
        for entry in self.project.read(cx).children(dir, cx) {
            let is_expanded = entry.is_dir && expanded.contains(&entry.path);
            rows.push(ProjectTreeRow {
                path: entry.path.clone(),
                name: entry.name,
                depth,
                is_dir: entry.is_dir,
                expanded: is_expanded,
                is_new: false,
                git_status: entry.git_status,
            });
            if is_expanded && !matches!(entry.git_status, Some(FileStatus::Ignored)) {
                self.collect_children(&entry.path, depth + 1, expanded, rows, cx);
            }
        }
    }

    /// 从 git 状态刷新行的忽略/颜色信息（git 事件驱动，不重扫目录）。
    fn refresh_git_statuses(&mut self, cx: &mut Context<Self>) {
        let entries: Vec<(PathBuf, bool)> = self
            .state
            .borrow()
            .rows
            .iter()
            .map(|row| (row.path.clone(), row.is_dir))
            .collect();
        let statuses = self.project.read(cx).git_statuses_for_rows(&entries, cx);
        for row in &mut self.state.borrow_mut().rows {
            row.git_status = statuses.get(&row.path).cloned();
        }
        cx.notify();
    }

    /// 重命名后迁移树状态（根/展开/选中/活动路径）并重建行模型。
    fn apply_rename(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.root.take() else {
            return;
        };
        self.root = Some(translate_path(&root, from, to));
        let mut state = self.state.borrow_mut();
        state.expanded = state
            .expanded
            .drain()
            .map(|path| translate_path(&path, from, to))
            .collect();
        state.selected = state
            .selected
            .take()
            .map(|path| translate_path(&path, from, to));
        self.active_path = self
            .active_path
            .take()
            .map(|path| translate_path(&path, from, to));
        drop(state);
        self.rebuild_rows(cx);
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

    /// 更换项目根目录（项目根被外部重命名时由 Workspace 调用）。
    pub(crate) fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root.as_ref() == Some(&root) {
            return;
        }
        self.root = Some(root.clone());
        let mut state = self.state.borrow_mut();
        state.expanded.clear();
        state.expanded.insert(root);
        state.selected = None;
        self.active_path = None;
        drop(state);
        // 行集合全变：重建时现查 git 状态，无需单独补齐。
        self.rebuild_rows(cx);
        cx.notify();
    }

    /// 刷新行模型；同时从设置读取最新的扫描排除名单并重建过滤规则。
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let exclusions = SettingsStore::file_scan_exclusions(cx);
        self.project
            .update(cx, |project, _| project.set_exclusions(&exclusions));
        self.rebuild_rows(cx);
        cx.notify();
    }

    /// 将活动文件标记，并在它不在视口内时滚动到可见区域。
    pub(crate) fn reveal_active_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let index = {
            let Some(root) = &self.root else {
                self.active_path = None;
                return;
            };
            let Some(path) = path.filter(|path| path.starts_with(root)) else {
                self.active_path = None;
                return;
            };
            let mut state = self.state.borrow_mut();
            let mut ancestor = path.parent();
            while let Some(directory) = ancestor.filter(|directory| directory.starts_with(root)) {
                state.expanded.insert(directory.to_path_buf());
                if directory == root {
                    break;
                }
                ancestor = directory.parent();
            }
            self.active_path = Some(path.to_path_buf());
            state.select(path.to_path_buf());
            drop(state);
            self.rebuild_rows(cx);
            self.state
                .borrow()
                .rows
                .iter()
                .position(|row| row.path == path)
        };
        if let Some(index) = index {
            self.scroll_handle
                .scroll_to_item(index, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn rows_and_len(&self) -> Vec<ProjectTreeRow> {
        self.state.borrow().rows().to_vec()
    }

    /// 保持键盘选中项可见；仍在视口内时不改变当前滚动位置。
    fn scroll_to_selection(&self) {
        if let Some(index) = self.state.borrow().selected_idx() {
            self.scroll_handle
                .scroll_to_item(index, ScrollStrategy::Center);
        }
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
        self.state.borrow_mut().select_up();
        self.scroll_to_selection();
        window.refresh();
    }
    fn handle_tree_select_next(
        &mut self,
        _: &TreeSelectNext,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.state.borrow_mut().select_down();
        self.scroll_to_selection();
        window.refresh();
    }
    fn handle_tree_collapse(
        &mut self,
        _: &TreeCollapse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rebuild = self.state.borrow_mut().collapse_selection();
        if rebuild {
            self.rebuild_rows(cx);
        }
        self.scroll_to_selection();
        window.refresh();
    }
    fn handle_tree_expand(&mut self, _: &TreeExpand, window: &mut Window, cx: &mut Context<Self>) {
        let rebuild = self.state.borrow_mut().expand_selection();
        if rebuild {
            self.rebuild_rows(cx);
        }
        self.scroll_to_selection();
        window.refresh();
    }
    /// 激活选中行或以临时标签打开的共享逻辑：目录→展开/折叠；文件→打开。
    ///
    /// `focus_opened_item` 决定打开文件后是否把焦点交给编辑器：双击/键盘 enter 为 `true`（激活），鼠标单击为 `false`（临时标签，焦点留在项目树）。
    fn activate_selected(
        &mut self,
        focus_opened_item: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (path, is_dir) = {
            let state = self.state.borrow();
            match state.selected_idx() {
                Some(idx) => (Some(state.rows[idx].path.clone()), state.rows[idx].is_dir),
                None => (None, false),
            }
        };
        let Some(path) = path else {
            return;
        };
        self.state.borrow_mut().select(path.clone());
        if is_dir {
            self.state.borrow_mut().toggle_expand(&path);
            self.rebuild_rows(cx);
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
            state.selected_idx().map(|index| state.rows[index].clone())
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
            let Some(index) = state.selected_idx() else {
                return;
            };
            (state.rows[index].path.clone(), index)
        };
        if self.root.as_ref() == Some(&path) {
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
        self.rebuild_rows(cx);
        let mut state = self.state.borrow_mut();
        // 删除后选中原位置的下一个条目；删除的是最后一项时落在新的最后一项。
        if !state.rows.is_empty() {
            state.selected = Some(state.rows[index.min(state.rows.len() - 1)].path.clone());
        }
    }

    fn begin_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_state.is_some() {
            return;
        }
        let row = {
            let state = self.state.borrow();
            state.selected_idx().map(|index| state.rows[index].clone())
        };
        let Some(row) = row else {
            return;
        };
        let parent = if row.is_dir {
            {
                let mut state = self.state.borrow_mut();
                state.expanded.insert(row.path.clone());
            }
            // 展开父目录产生新行，重建时现查 git 状态。
            self.rebuild_rows(cx);
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
                self.apply_rename(&source, &destination, cx);
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
                drop(state);
                self.rebuild_rows(cx);
                self.state.borrow_mut().selected = Some(new_entry.path.clone());
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

impl gpui::Render for ProjectTreePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        self.state.borrow_mut().ensure_selected();
        let content = if self.root.is_none() {
            render_empty_state(cx).into_any_element()
        } else {
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
                active_path: self.active_path.clone(),
            };
            render_list(
                &self.scroll_handle,
                &self.scrollbar,
                len,
                is_focused,
                render_context,
            )
            .into_any_element()
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
            .child(content)
    }
}

/// 无 worktree 的空态提示。
fn render_empty_state(cx: &App) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .text_color(color::current(cx).text_placeholder)
        .child("没有打开的项目")
}

// ── 私有渲染辅助函数 ────────────────────────────────────────────────

fn render_list(
    scroll_handle: &UniformListScrollHandle,
    scrollbar: &Scrollbar<UniformListScrollHandle>,
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
                let marked = !row.is_new && render_context.active_path.as_ref() == Some(&row.path);
                render_row(row, sel, marked, is_focused, render_context.clone(), cx)
                    .into_any_element()
            })
            .collect()
    })
    .size_full()
    .track_scroll(handle)
    .with_decoration(scrollbar.clone())
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

    tree::render_row_base(depth, &row.path, is_dir, row.expanded, content, cx)
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
                            tree.state.borrow_mut().select(path.clone());
                            match event.click_count {
                                // 单击：目录展开/折叠、文件以临时标签打开（焦点留在项目树）；
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

impl Panel for ProjectTreePanel {
    fn icon() -> &'static str {
        "icons/file_tree.svg"
    }
    fn label() -> &'static str {
        "项目树"
    }
    fn persistent_name() -> &'static str {
        "project-tree"
    }
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

// ── 内部类型 ────────────────────────────────────────────────────────

#[derive(Clone)]
struct ProjectTreeRenderContext {
    state: Rc<RefCell<TreeState<PathBuf, ProjectTreeRow>>>,
    rows: Rc<[ProjectTreeRow]>,
    focus: gpui::FocusHandle,
    /// 条目点击直接调用 Entity 方法（对齐 Zed 的 `cx.listener` 路径），
    /// 不依赖 dispatch_action 的焦点链分发。
    weak: WeakEntity<ProjectTreePanel>,
    edit_state: Option<EditState>,
    entry_name_editor: Entity<Editor>,
    /// 活动文件标记（渲染时快照，与选中行独立）。
    active_path: Option<PathBuf>,
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

impl TreeRow for ProjectTreeRow {
    fn is_dir(&self) -> bool {
        self.is_dir
    }
    fn depth(&self) -> usize {
        self.depth
    }
    fn expanded(&self) -> bool {
        self.expanded
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
    fn rows_are_cached_until_rebuild_reinjects_them() {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let file = directory.path().join("cached.txt");
        std::fs::write(&file, "content").expect("应创建测试文件");
        let mut state = TreeState::new(|row: &ProjectTreeRow| Some(row.path.clone()));
        let root = directory.path().to_path_buf();
        let rows = vec![
            ProjectTreeRow {
                path: root.clone(),
                name: root
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                depth: 0,
                is_dir: true,
                expanded: true,
                is_new: false,
                git_status: None,
            },
            ProjectTreeRow {
                path: file.clone(),
                name: "cached.txt".to_string(),
                depth: 1,
                is_dir: false,
                expanded: false,
                is_new: false,
                git_status: None,
            },
        ];
        state.replace_rows(rows);

        // 渲染读取的是注入的缓存：文件系统变化不影响行模型。
        std::fs::remove_file(&file).expect("应删除测试文件");
        assert!(state.rows().iter().any(|row| row.path == file));

        // 只有显式重建（由 ProjectTreePanel 调 worktree 遍历）才会反映文件系统。
        state.replace_rows(vec![ProjectTreeRow {
            path: root,
            name: "root".to_string(),
            depth: 0,
            is_dir: true,
            expanded: true,
            is_new: false,
            git_status: None,
        }]);
        assert!(!state.rows().iter().any(|row| row.path == file));
    }

    #[gpui::test]
    fn revealing_active_file_expands_ancestors_and_keeps_mark_separate_from_selection(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let nested = directory.path().join("src").join("feature");
        std::fs::create_dir_all(&nested).expect("应创建嵌套目录");
        let file = nested.join("mod.rs");
        std::fs::write(&file, "content").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
        let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));

        tree.update(cx, |tree, cx| {
            tree.reveal_active_path(Some(file.clone()), cx)
        });
        cx.read_entity(&tree, |tree, _| {
            assert_eq!(tree.active_path.as_deref(), Some(file.as_path()));
            assert_eq!(
                tree.state.borrow().selected.as_deref(),
                Some(file.as_path())
            );
            assert!(tree.state.borrow().rows.iter().any(|row| row.path == file));
            assert!(
                tree.state
                    .borrow()
                    .expanded
                    .contains(&directory.path().join("src"))
            );
            assert!(tree.state.borrow().expanded.contains(&nested));

            // 键盘游标移动不应改变活动文件标记。
            tree.state.borrow_mut().select_up();
            assert_ne!(
                tree.state.borrow().selected.as_deref(),
                Some(file.as_path())
            );
            assert_eq!(tree.active_path.as_deref(), Some(file.as_path()));
        });
    }

    #[gpui::test]
    fn revealing_path_outside_project_clears_active_mark(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let file = directory.path().join("active.txt");
        std::fs::write(&file, "content").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
        let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));

        tree.update(cx, |tree, cx| {
            tree.reveal_active_path(Some(file.clone()), cx)
        });
        tree.update(cx, |tree, cx| {
            tree.reveal_active_path(Some(PathBuf::from("/outside/project.txt")), cx);
        });
        cx.read_entity(&tree, |tree, _| {
            assert!(tree.active_path.is_none());
        });
    }

    #[gpui::test]
    fn applying_directory_rename_migrates_tree_paths(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let old_directory = directory.path().join("old");
        let old_file = old_directory.join("mod.rs");
        std::fs::create_dir(&old_directory).expect("应创建待重命名目录");
        std::fs::write(&old_file, "content").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
        let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));

        // reveal 展开祖先并标记活动文件。
        tree.update(cx, |tree, cx| {
            tree.reveal_active_path(Some(old_file.clone()), cx)
        });

        let new_directory = directory.path().join("new");
        std::fs::rename(&old_directory, &new_directory).expect("应重命名测试目录");
        tree.update(cx, |tree, cx| {
            tree.apply_rename(&old_directory, &new_directory, cx)
        });

        let new_file = new_directory.join("mod.rs");
        cx.read_entity(&tree, |tree, _| {
            assert!(tree.state.borrow().expanded.contains(&new_directory));
            assert_eq!(
                tree.state.borrow().selected.as_deref(),
                Some(new_file.as_path())
            );
            assert_eq!(tree.active_path.as_deref(), Some(new_file.as_path()));
            assert!(
                tree.state
                    .borrow()
                    .rows
                    .iter()
                    .any(|row| row.path == new_file)
            );
        });
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
            let mut tree = ProjectTreePanel::new(project.clone(), cx);
            tree.set_on_open_file(Rc::new(move |_, _, _, _| {
                callback_count.set(callback_count.get() + 1);
            }));
            tree.state.borrow_mut().select(selected_file.clone());
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

        // 记录每次打开回调的 focus_opened_item：单击临时打开应为 false，双击激活应为 true。
        let open_count = Rc::new(Cell::new(0));
        let last_focus_opened = Rc::new(Cell::new(true));
        let callback_count = Rc::clone(&open_count);
        let callback_focus = Rc::clone(&last_focus_opened);

        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (_tree, cx) = cx.add_window_view(move |_, cx| {
            let mut tree = ProjectTreePanel::new(project.clone(), cx);
            tree.set_on_open_file(Rc::new(move |_, focus_opened_item, _, _| {
                callback_count.set(callback_count.get() + 1);
                callback_focus.set(focus_opened_item);
            }));
            // 展开根目录使文件行可见；不聚焦项目树（模拟焦点在别处）。
            let root = tree.root.clone().expect("测试项目应包含根目录");
            tree.state.borrow_mut().expanded.insert(root);
            tree.rebuild_rows(cx);
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
            "单击文件应打开临时标签但焦点留在项目树（focus_opened_item=false）"
        );
    }

    #[gpui::test]
    fn rename_actions_edit_and_confirm_the_selected_row(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let old_path = directory.path().join("old.txt");
        let new_path = directory.path().join("new.txt");
        std::fs::write(&old_path, "content").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
        let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
        tree.update(cx, |tree, _| {
            tree.set_on_rename(Rc::new(|from, to, _| {
                std::fs::rename(from, to)?;
                Ok(())
            }));
            tree.state.borrow_mut().select(old_path.clone());
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
        let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
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

                tree.state
                    .borrow_mut()
                    .select(directory.path().to_path_buf());
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
        let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
        let trashed = Rc::new(RefCell::new(None));
        let trashed_path = Rc::clone(&trashed);
        tree.update(cx, |tree, _| {
            tree.set_on_trash(Rc::new(move |path, _, _| {
                std::fs::remove_file(&path)?;
                *trashed_path.borrow_mut() = Some(path);
                Ok(())
            }));
            tree.state.borrow_mut().select(trashed_file.clone());
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
        let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
        let called = Rc::new(Cell::new(false));
        let callback_called = Rc::clone(&called);
        tree.update(cx, |tree, _| {
            tree.set_on_trash(Rc::new(move |_, _, _| {
                callback_called.set(true);
                Ok(())
            }));
            tree.state
                .borrow_mut()
                .select(directory.path().to_path_buf());
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
        let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
        tree.update(cx, |tree, _| {
            tree.set_on_trash(Rc::new(|path, _, _| {
                std::fs::remove_file(path)?;
                Ok(())
            }));
            tree.state.borrow_mut().select(only_file.clone());
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
        let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
        cx.run_until_parked();

        // 修改文件 → git 增量刷新 → StatusesChanged 事件 → 行颜色更新。
        let file = root.join("tracked.txt");
        std::fs::write(&file, "已修改\n").expect("应修改文件");
        project.update(cx, |project, cx| {
            project.git_store().update(cx, |store, cx| {
                store.refresh_statuses_for_paths(std::slice::from_ref(&file), cx);
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
        let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
        cx.run_until_parked();

        // 修改目录内文件 → 增量刷新 → 目录行聚合为 modified。
        std::fs::write(&src_file, "fn main() { println!(); }\n").expect("应修改文件");
        project.update(cx, |project, cx| {
            project.git_store().update(cx, |store, cx| {
                store.refresh_statuses_for_paths(std::slice::from_ref(&src_file), cx);
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
        let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
        cx.run_until_parked();

        // 展开 sub 目录（模拟 handle_tree_expand 的行重建路径）。
        tree.update(cx, |tree, cx| {
            tree.state.borrow_mut().expanded.insert(sub.clone());
            tree.rebuild_rows(cx);
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
        let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
        cx.run_until_parked();

        // 模拟鼠标点击：选中行后激活（与键盘 enter 同一 handler）。
        cx.add_window_view(|window, cx| {
            tree.update(cx, |tree, _| {
                tree.state.borrow_mut().select(sub.clone());
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

    /// 无 worktree 的空项目：面板照常构造，root 为空、行模型为空（渲染空态提示）。
    #[gpui::test]
    fn empty_project_has_no_root_and_empty_rows(cx: &mut TestAppContext) {
        let project = cx.update(|cx| cx.new(|cx| Project::empty(cx)));
        let (tree, cx) = cx.add_window_view(move |_, cx| ProjectTreePanel::new(project, cx));

        cx.read_entity(&tree, |tree, _| {
            assert!(tree.root.is_none());
            assert!(tree.state.borrow().rows.is_empty());
        });
    }
}
