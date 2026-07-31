//! ProjectTree —— 项目文件树 Entity 组件。
//!
//! 持有 `Rc<RefCell<ProjectTreeState>>` 管理展开/选中状态和缓存行模型。
//! 文件系统只在刷新模型时读取；渲染与键盘导航只消费缓存。

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    Context, Div, MouseButton, ScrollStrategy, UniformListScrollHandle, Window, actions, div,
    prelude::*, uniform_list,
};

use crate::ui::tree;
use crate::workspace::Panel;
use zcv_theme::color;

actions!(
    project_tree,
    [
        TreeSelectPrev,
        TreeSelectNext,
        TreeCollapse,
        TreeExpand,
        TreeActivate
    ]
);

/// 打开文件回调
pub(crate) type OnOpenFile = Rc<dyn Fn(PathBuf, bool, &mut Window, &mut gpui::App)>;

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
    on_open_file: Option<OnOpenFile>,
}

impl ProjectTree {
    pub(crate) fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        Self {
            focus,
            root: root.clone(),
            state: Rc::new(RefCell::new(ProjectTreeState::new(root))),
            scroll_handle: UniformListScrollHandle::default(),
            on_open_file: None,
        }
    }

    /// 设置打开文件的回调（由 Workspace 在创建后调用）。
    pub(crate) fn set_on_open_file(&mut self, callback: OnOpenFile) {
        self.on_open_file = Some(callback);
    }

    /// 更换项目根目录。
    pub(crate) fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.root = root.clone();
        self.state.borrow_mut().set_root(root.clone());
        cx.notify();
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
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
            match state.selected_idx(&rows) {
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
}

impl gpui::Render for ProjectTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        self.state.borrow_mut().ensure_selected();
        let len = self.state.borrow().visible_rows().len();
        let is_focused = self.focus.contains_focused(window, cx);
        let focus = self.focus.clone();
        let on_open = self.on_open_file.clone();

        div()
            .size_full()
            .track_focus(&self.focus)
            .key_context("ProjectTree")
            .tab_index(0)
            .on_action(cx.listener(Self::handle_tree_select_prev))
            .on_action(cx.listener(Self::handle_tree_select_next))
            .on_action(cx.listener(Self::handle_tree_collapse))
            .on_action(cx.listener(Self::handle_tree_expand))
            .on_action(cx.listener(Self::handle_tree_activate))
            .child(render_list(
                &self.state,
                &self.scroll_handle,
                len,
                is_focused,
                focus,
                on_open,
            ))
    }
}

// ── 私有渲染辅助函数 ────────────────────────────────────────────────

fn render_list(
    state: &Rc<RefCell<ProjectTreeState>>,
    scroll_handle: &UniformListScrollHandle,
    len: usize,
    is_focused: bool,
    focus: gpui::FocusHandle,
    on_open_file: Option<OnOpenFile>,
) -> gpui::UniformList {
    let tree_rc = Rc::clone(state);
    let handle = scroll_handle.clone();

    uniform_list("project-tree-list", len, move |range, _, _| {
        let state = tree_rc.borrow();
        let rows = state.visible_rows();
        range
            .filter_map(|i| rows.get(i))
            .map(|row| {
                let sel = state.selected.as_ref() == Some(&row.path);
                let marked = state.active_path.as_ref() == Some(&row.path);
                render_row(
                    row,
                    Rc::clone(&tree_rc),
                    sel,
                    marked,
                    is_focused,
                    focus.clone(),
                    &on_open_file,
                )
                .into_any_element()
            })
            .collect()
    })
    .size_full()
    .track_scroll(handle)
}

fn render_row(
    row: &ProjectTreeRow,
    state: Rc<RefCell<ProjectTreeState>>,
    sel: bool,
    marked: bool,
    focused: bool,
    focus: gpui::FocusHandle,
    on_open_file: &Option<OnOpenFile>,
) -> Div {
    let path = row.path.clone();
    let is_dir = row.is_dir;
    let depth = row.depth;
    let name = row.name.clone();
    let on_open = on_open_file.clone();

    tree::render_row_base(depth, is_dir, row.expanded, &name)
        .cursor_pointer()
        .when(marked, |el| el.bg(color::current().element_selected))
        .hover(|style| style.bg(color::current().element_hover))
        .when(sel && focused, |el| el.child(tree::selection_border()))
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            if is_dir {
                window.focus(&focus);
            }
            activate_node(
                &state,
                &path,
                is_dir,
                event.click_count > 1,
                &on_open,
                window,
                cx,
            );
            cx.stop_propagation();
        })
}

// ── 内部类型 ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ProjectTreeRow {
    path: PathBuf,
    name: String,
    depth: usize,
    is_dir: bool,
    expanded: bool,
}

struct ProjectTreeState {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    active_path: Option<PathBuf>,
    rows: Vec<ProjectTreeRow>,
}

impl ProjectTreeState {
    fn new(root: PathBuf) -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        let mut state = Self {
            root,
            expanded,
            selected: None,
            active_path: None,
            rows: Vec::new(),
        };
        state.refresh_rows();
        state
    }

    /// 更换根目录，重置展开和选中状态。
    fn set_root(&mut self, root: PathBuf) {
        self.root = root;
        self.expanded.clear();
        self.expanded.insert(self.root.clone());
        self.selected = None;
        self.active_path = None;
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
        });

        if root_expanded {
            self.collect_children(&self.root, 1, &mut rows);
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

    fn collect_children(&self, dir: &Path, depth: usize, rows: &mut Vec<ProjectTreeRow>) {
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
            let is_expanded = self.expanded.contains(&entry);
            rows.push(ProjectTreeRow {
                path: entry.clone(),
                name,
                depth,
                is_dir,
                expanded: is_expanded,
            });
            if is_dir && is_expanded {
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
}

impl Panel for ProjectTree {
    fn persistent_name() -> &'static str {
        "ProjectTree"
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
