use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    Context, Div, MouseButton, UniformListScrollHandle, Window, actions, div, prelude::*,
    uniform_list,
};
use zcv_engine::{Buffer, BufferConfig, BufferOrigin};

use crate::editor::ViewRegistry;
use crate::shared::tree;
use crate::theme::color;
use crate::workbench::LayoutRef;

actions!(
    project_tree,
    [TreeUp, TreeDown, TreeLeft, TreeRight, TreeSpace]
);

// ── 内部状态 ──────────────────────────────────────────────────────────

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
}

impl ProjectTreeState {
    fn new(root: PathBuf) -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        Self {
            root,
            expanded,
            selected: None,
        }
    }

    fn visible_rows(&self) -> Vec<ProjectTreeRow> {
        let mut rows = Vec::new();
        self.collect_children(&self.root, 0, &mut rows);
        rows
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
    }

    fn select(&mut self, path: &Path) {
        self.selected = Some(path.to_path_buf());
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

// ── Entity ──────────────────────────────────────────────────────────

pub(crate) struct ProjectTree {
    focus: gpui::FocusHandle,
    state: Rc<RefCell<ProjectTreeState>>,
    scroll_handle: UniformListScrollHandle,
}

impl ProjectTree {
    pub(crate) fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            state: Rc::new(RefCell::new(ProjectTreeState::new(root))),
            scroll_handle: UniformListScrollHandle::default(),
        }
    }

    fn rows_and_len(&self) -> Vec<ProjectTreeRow> {
        self.state.borrow().visible_rows()
    }

    fn handle_tree_up(&mut self, _: &TreeUp, window: &mut Window, _: &mut Context<Self>) {
        let rows = self.rows_and_len();
        self.state.borrow_mut().select_up(&rows);
        window.refresh();
    }
    fn handle_tree_down(&mut self, _: &TreeDown, window: &mut Window, _: &mut Context<Self>) {
        let rows = self.rows_and_len();
        self.state.borrow_mut().select_down(&rows);
        window.refresh();
    }
    fn handle_tree_left(&mut self, _: &TreeLeft, window: &mut Window, _: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let rows = state.visible_rows();
        let Some(idx) = state.selected_idx(&rows) else {
            return;
        };
        let row = &rows[idx];
        if row.is_dir && row.expanded {
            state.expanded.remove(&row.path);
        } else if row.depth > 0 {
            let pd = row.depth - 1;
            if let Some(pi) = rows[..idx].iter().rposition(|r| r.is_dir && r.depth == pd) {
                state.selected = Some(rows[pi].path.clone());
            }
        }
        window.refresh();
    }
    fn handle_tree_right(&mut self, _: &TreeRight, window: &mut Window, _: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let rows = state.visible_rows();
        let Some(idx) = state.selected_idx(&rows) else {
            return;
        };
        let row = &rows[idx];
        if row.is_dir && !row.expanded {
            state.expanded.insert(row.path.clone());
        } else {
            state.select_down(&rows);
        }
        window.refresh();
    }
    fn handle_tree_space(&mut self, _: &TreeSpace, window: &mut Window, _: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let rows = state.visible_rows();
        if let Some(idx) = state.selected_idx(&rows) {
            let row = &rows[idx];
            if row.is_dir {
                state.toggle_expand(&row.path);
            }
        }
        window.refresh();
    }
}

impl gpui::Render for ProjectTree {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let len = self.state.borrow().visible_rows().len();

        div()
            .size_full()
            .track_focus(&self.focus)
            .key_context("FileTree")
            .tab_index(0)
            .on_action(cx.listener(Self::handle_tree_up))
            .on_action(cx.listener(Self::handle_tree_down))
            .on_action(cx.listener(Self::handle_tree_left))
            .on_action(cx.listener(Self::handle_tree_right))
            .on_action(cx.listener(Self::handle_tree_space))
            .child(render_list(&self.state, &self.scroll_handle, len))
    }
}

fn render_list(
    state: &Rc<RefCell<ProjectTreeState>>,
    scroll_handle: &UniformListScrollHandle,
    len: usize,
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
                render_row(row, Rc::clone(&tree_rc), sel).into_any_element()
            })
            .collect()
    })
    .size_full()
    .track_scroll(handle)
}

fn render_row(row: &ProjectTreeRow, state: Rc<RefCell<ProjectTreeState>>, sel: bool) -> Div {
    let path = row.path.clone();
    let is_dir = row.is_dir;
    let depth = row.depth;
    let name = row.name.clone();
    let bg = if sel {
        color::current().gray.s[3]
    } else {
        gpui::rgba(0)
    };

    tree::render_row_base(depth, is_dir, row.expanded, &name)
        .bg(bg)
        .cursor_pointer()
        .when(sel, |el| el.child(tree::selection_border()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            let mut s = state.borrow_mut();
            s.select(&path);
            if is_dir {
                s.toggle_expand(&path);
            } else {
                // 文件 → 在编辑器中打开
                open_file_in_editor(&path, window, cx);
            }
            window.refresh();
            cx.stop_propagation();
        })
}

/// 打开文件并在编辑器中显示。
fn open_file_in_editor(path: &Path, _window: &mut Window, cx: &mut gpui::App) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let buffer = Rc::new(RefCell::new(
        Buffer::with_origin(
            BufferOrigin::external(path.to_string_lossy().to_string()),
            content,
            BufferConfig::default(),
        )
        .expect("创建 Buffer 不应失败"),
    ));

    let mut view_id = None;
    cx.update_global::<ViewRegistry, _>(|reg, _| {
        view_id = Some(reg.register(path.to_path_buf(), buffer));
    });
    let view_id = view_id.expect("应分配到有效 ViewId");

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    if let Some(layout_ref) = cx.try_global::<LayoutRef>() {
        if let Some(ctrl) = layout_ref.0.upgrade() {
            ctrl.borrow_mut().open_file(view_id, &file_name);
        }
    }
}
