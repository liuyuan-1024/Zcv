//! ProjectTree —— 项目文件树 Entity 组件。
//!
//! 持有 `Rc<RefCell<ProjectTreeState>>` 管理展开/选中状态和缓存行模型。
//! 文件系统只在刷新模型时读取；渲染与键盘导航只消费缓存。

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use globset::{Glob, GlobSet, GlobSetBuilder};
use gpui::{
    Context, Div, Entity, KeyContext, MouseButton, ScrollStrategy, UniformListScrollHandle, Window,
    actions, div, prelude::*, uniform_list,
};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::settings::SettingsStore;
use crate::ui::tree;
use crate::workspace::Panel;
use zcv_editor::Editor;
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

/// 选中并激活项目树中的节点（目录→展开/折叠，文件→打开）。
fn activate_node(
    state: &Rc<RefCell<ProjectTreeState>>,
    path: &Path,
    is_dir: bool,
    focus_opened_item: bool,
    on_open_file: &Option<OnOpenFile>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    state.borrow_mut().select(path);
    if is_dir {
        state.borrow_mut().toggle_expand(path);
    } else if let Some(callback) = on_open_file {
        callback(path.to_path_buf(), focus_opened_item, window, cx);
    }
    window.refresh();
}

// ── Entity ──────────────────────────────────────────────────────────

pub(crate) struct ProjectTree {
    pub focus: gpui::FocusHandle,
    /// 当前项目根目录路径。
    root: PathBuf,
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
    pub(crate) fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let entry_name_editor = cx.new(Editor::single_line);
        cx.observe(&entry_name_editor, |_, _, cx| cx.notify())
            .detach();
        let exclusions = SettingsStore::file_scan_exclusions(cx);
        let mut state = ProjectTreeState::new(root.clone());
        state.set_filter(&exclusions);
        Self {
            focus,
            root,
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
        cx.notify();
    }

    /// 刷新行模型；同时从设置读取最新的扫描排除名单并重建过滤规则。
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let exclusions = SettingsStore::file_scan_exclusions(cx);
        self.state.borrow_mut().set_filter(&exclusions);
        self.state.borrow_mut().refresh_rows();
        cx.notify();
    }

    /// 将活动文件标记并强制滚动到项目树中央。
    pub(crate) fn reveal_active_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let index = self.state.borrow_mut().reveal_active_path(path.as_deref());
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
                is_ignored: false,
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
    fn handle_tree_expand(&mut self, _: &TreeExpand, window: &mut Window, _: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let rows = state.visible_rows().to_vec();
        let Some(idx) = state.selected_idx(&rows) else {
            return;
        };
        let row = &rows[idx];
        if row.is_dir && !row.expanded {
            state.expanded.insert(row.path.clone());
            state.refresh_rows();
        } else {
            state.select_down(&rows);
        }
        window.refresh();
    }
    fn handle_tree_activate(
        &mut self,
        _: &TreeActivate,
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
        activate_node(
            &self.state,
            &path,
            is_dir,
            true,
            &self.on_open_file,
            window,
            cx,
        );
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
            let mut state = self.state.borrow_mut();
            state.expanded.insert(row.path.clone());
            state.refresh_rows();
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
            on_open_file: self.on_open_file.clone(),
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

    uniform_list("project-tree-list", len, move |range, _, _| {
        let state = render_context.state.borrow();
        let rows = &render_context.rows;
        range
            .filter_map(|i| rows.get(i))
            .map(|row| {
                let sel = row.is_new || state.selected.as_ref() == Some(&row.path);
                let marked = !row.is_new && state.active_path.as_ref() == Some(&row.path);
                render_row(row, sel, marked, is_focused, render_context.clone()).into_any_element()
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
                    .border_color(color::current().status_error)
            })
            .child(render_context.entry_name_editor.clone())
    } else {
        // gitignored 条目淡显。
        div()
            .flex_1()
            .overflow_hidden()
            .truncate()
            .when(row.is_ignored, |element| {
                element.text_color(color::current().text_muted)
            })
            .child(name)
    };

    tree::render_row_base(depth, is_dir, row.expanded, content)
        .cursor_pointer()
        .when(marked, |el| el.bg(color::current().element_selected))
        .hover(|style| style.bg(color::current().element_hover))
        .when(sel && focused, |el| el.child(tree::selection_border()))
        .when(!is_editing, |row| {
            row.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                if is_dir {
                    window.focus(&render_context.focus);
                }
                activate_node(
                    &render_context.state,
                    &path,
                    is_dir,
                    event.click_count > 1,
                    &render_context.on_open_file,
                    window,
                    cx,
                );
                cx.stop_propagation();
            })
        })
}

impl Panel for ProjectTree {
    fn icon() -> &'static str {
        "icons/panels/project_tree.svg"
    }
    fn label() -> &'static str {
        "项目树"
    }
    fn action_name() -> &'static str {
        "dock::ToggleProjectTree"
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
    on_open_file: Option<OnOpenFile>,
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
    /// 命中 gitignore 规则（目录不展开内容、文件淡显）。
    is_ignored: bool,
}

/// 项目树的过滤规则：扫描排除（glob 名单）+ gitignore。
///
/// file_scan_exclusions 命中的条目根本不在树中加载；
/// gitignore 命中的条目显示但标记为忽略。
struct TreeFilter {
    /// 用户配置的扫描排除 glob。
    exclusions: GlobSet,
    /// 项目根及子目录链上的 .gitignore 匹配器。
    gitignore: Option<Arc<Gitignore>>,
}

impl TreeFilter {
    fn new(root: &Path, exclusions: &[String]) -> Self {
        let mut builder = GlobSetBuilder::new();
        for glob in exclusions {
            if let Ok(glob) = Glob::new(glob) {
                builder.add(glob);
            }
        }
        let root_ignore = root.join(".gitignore");
        let root_files = if root_ignore.is_file() {
            vec![root_ignore]
        } else {
            Vec::new()
        };
        Self {
            exclusions: builder.build().unwrap_or_default(),
            gitignore: Self::build_gitignore(root, &root_files),
        }
    }

    /// 用从项目根到目标目录链上的全部 .gitignore 构建匹配器。
    ///
    /// 每遇到一个目录的 .gitignore 就重建一次；
    /// 链上文件数通常只有 1-2 个，且只在刷新行模型时调用，成本可控。
    fn build_gitignore(root: &Path, ignore_files: &[PathBuf]) -> Option<Arc<Gitignore>> {
        if ignore_files.is_empty() {
            return None;
        }
        let mut builder = GitignoreBuilder::new(root);
        for file in ignore_files {
            if let Some(error) = builder.add(file) {
                log::warn!("无法加载 .gitignore {:?}：{error}", file);
            }
        }
        builder.build().ok().map(Arc::new)
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
}

impl ProjectTreeState {
    fn new(root: PathBuf) -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        let filter = TreeFilter::new(&root, &[]);
        let mut state = Self {
            root,
            expanded,
            selected: None,
            active_path: None,
            rows: Vec::new(),
            exclusions: Vec::new(),
            filter,
        };
        state.refresh_rows();
        state
    }

    /// 注入扫描排除规则并重建过滤（由 ProjectTree 在创建或设置变化时调用）。
    fn set_filter(&mut self, exclusions: &[String]) {
        self.exclusions = exclusions.to_vec();
        self.filter = TreeFilter::new(&self.root, exclusions);
    }

    /// 更换根目录，重置展开和选中状态。
    fn set_root(&mut self, root: PathBuf) {
        self.root = root;
        self.expanded.clear();
        self.expanded.insert(self.root.clone());
        self.selected = None;
        self.active_path = None;
        self.filter = TreeFilter::new(&self.root, &self.exclusions);
        self.refresh_rows();
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
            is_ignored: false,
        });

        if root_expanded {
            let root = self.root.clone();
            let gitignore = self.filter.gitignore.clone();
            self.collect_children(&root, 1, &mut rows, gitignore, Vec::new(), false);
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

    /// 递归收集目录子项；`gitignore` 是父链上传下来的匹配器，
    /// `ignore_files` 是已发现的 .gitignore 文件链，`inherited_ignored` 表示父目录已被 gitignore 命中（子项全部继承忽略）。
    fn collect_children(
        &mut self,
        dir: &Path,
        depth: usize,
        rows: &mut Vec<ProjectTreeRow>,
        gitignore: Option<Arc<Gitignore>>,
        ignore_files: Vec<PathBuf>,
        inherited_ignored: bool,
    ) {
        // 该目录自己的 .gitignore 存在时，加入链并重建匹配器。
        let (gitignore, ignore_files) = {
            let dir_ignore = dir.join(".gitignore");
            if dir_ignore.is_file() {
                let mut files = ignore_files;
                files.push(dir_ignore);
                (TreeFilter::build_gitignore(&self.root, &files), files)
            } else {
                (gitignore, ignore_files)
            }
        };

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
            let is_ignored = inherited_ignored
                || gitignore
                    .as_ref()
                    .is_some_and(|g| g.matched(rel, is_dir).is_ignore());
            let is_expanded = self.expanded.contains(&entry);
            rows.push(ProjectTreeRow {
                path: entry.clone(),
                name,
                depth,
                is_dir,
                expanded: is_expanded,
                is_new: false,
                is_ignored,
            });
            // gitignored 目录不展开内容，避免 node_modules 这类目录撑爆行模型。
            if is_dir && is_expanded && !is_ignored {
                self.collect_children(
                    &entry,
                    depth + 1,
                    rows,
                    gitignore.clone(),
                    ignore_files.clone(),
                    is_ignored,
                );
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

    use gpui::{AppContext, KeyBinding, Render, TestAppContext};

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
    fn gitignored_directories_do_not_expand_and_files_are_marked() {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let node_modules = directory.path().join("node_modules");
        std::fs::create_dir_all(node_modules.join("pkg")).expect("应创建被忽略目录");
        std::fs::write(node_modules.join("pkg").join("index.js"), "// ignored")
            .expect("应创建被忽略文件");
        std::fs::write(
            directory.path().join(".gitignore"),
            "node_modules/\n*.log\n",
        )
        .expect("应写 .gitignore");
        std::fs::write(directory.path().join("app.log"), "log").expect("应创建日志文件");
        let visible = directory.path().join("main.js");
        std::fs::write(&visible, "console.log(1)").expect("应创建可见文件");

        let mut state = ProjectTreeState::new(directory.path().to_path_buf());
        state.expanded.insert(node_modules.clone());
        state.refresh_rows();

        let rows = state.visible_rows().to_vec();
        let nm = rows
            .iter()
            .find(|row| row.path == node_modules)
            .expect("node_modules 行应存在");
        assert!(nm.is_ignored);
        assert!(
            !rows
                .iter()
                .any(|row| row.path.starts_with(&node_modules.join("pkg"))),
            "被忽略目录不应展开内容"
        );
        assert!(
            rows.iter()
                .find(|row| row.path == directory.path().join("app.log"))
                .is_some_and(|row| row.is_ignored),
            "*.log 文件应被标记为忽略"
        );
        assert!(
            !rows
                .iter()
                .find(|row| row.path == visible)
                .expect("可见文件行应存在")
                .is_ignored
        );
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
        state.expanded.insert(sub.clone());
        state.refresh_rows();

        let rows = state.visible_rows().to_vec();
        assert!(
            rows.iter()
                .find(|row| row.path == nested)
                .expect("secret.txt 行应存在")
                .is_ignored,
            "嵌套 .gitignore 的规则应生效"
        );
        assert!(
            !rows
                .iter()
                .find(|row| row.path == visible)
                .expect("visible.txt 行应存在")
                .is_ignored
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

        let (tree, cx) = cx.add_window_view(move |_, cx| {
            cx.bind_keys([
                KeyBinding::new("enter", TreeRename, Some("ProjectTree && not_editing")),
                KeyBinding::new("space", TreeActivate, Some("ProjectTree && not_editing")),
            ]);
            let mut tree = ProjectTree::new(project_root, cx);
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
    fn rename_actions_edit_and_confirm_the_selected_row(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let old_path = directory.path().join("old.txt");
        let new_path = directory.path().join("new.txt");
        std::fs::write(&old_path, "content").expect("应创建测试文件");
        let tree = cx.new(|cx| ProjectTree::new(directory.path().to_path_buf(), cx));
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
        let tree = cx.new(|cx| ProjectTree::new(directory.path().to_path_buf(), cx));
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
        let tree = cx.new(|cx| ProjectTree::new(directory.path().to_path_buf(), cx));
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
        let tree = cx.new(|cx| ProjectTree::new(directory.path().to_path_buf(), cx));
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
        let tree = cx.new(|cx| ProjectTree::new(directory.path().to_path_buf(), cx));
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
}
