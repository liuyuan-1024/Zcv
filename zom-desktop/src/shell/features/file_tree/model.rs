//! 文件树运行模型。
//!
//! 负责项目目录树、展开状态、选中行、面板快照构造，以及从文件树激活文件。

use std::path::{Path, PathBuf};

use zom_view::ViewSet;
use zom_workspace::{BufferId, EntryKind, ProjectTree, Workspace};

use super::{FileTreeActivation, FileTreeRow, FileTreeState};

#[derive(Default)]
pub(crate) struct FileTreeModel {
    project_tree: Option<ProjectTree>,
    selected: Option<PathBuf>,
}

impl FileTreeModel {
    pub(crate) fn open_project(&mut self, root: PathBuf) {
        self.project_tree = match ProjectTree::new(root.clone()) {
            Ok(tree) => Some(tree),
            Err(error) => {
                eprintln!("读取项目目录失败：{}：{error}", root.display());
                None
            }
        };
        self.selected = None;
    }

    pub(crate) fn state(&self, workspace: &Workspace) -> FileTreeState {
        let Some(tree) = self.project_tree.as_ref() else {
            return FileTreeState::default();
        };
        let rows = tree
            .visible_rows()
            .into_iter()
            .map(|row| FileTreeRow {
                path: row.path.to_path_buf(),
                name: row.name.to_string(),
                depth: row.depth,
                kind: row.kind,
                expanded: row.expanded,
            })
            .collect();
        let active = workspace
            .active_buffer()
            .and_then(|buffer| buffer.path())
            .map(PathBuf::from);
        FileTreeState {
            rows,
            selected: self.selected.clone(),
            active,
        }
    }

    /// 焦点进入文件树时调用：若尚未选中任何行，默认落到第一行，让边框
    /// 立刻出现，无需用户先按一次 ↓。
    pub(crate) fn ensure_selection_initialized(&mut self) {
        if self.selected.is_some() {
            return;
        }
        let Some(tree) = self.project_tree.as_ref() else {
            return;
        };
        if let Some(first) = tree.visible_rows().first() {
            self.selected = Some(first.path.to_path_buf());
        }
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let Some(tree) = self.project_tree.as_ref() else {
            return;
        };
        let paths: Vec<PathBuf> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.path.to_path_buf())
            .collect();
        if paths.is_empty() {
            return;
        }
        let new_index = match self.selected.as_ref() {
            None => {
                if delta >= 0 {
                    0
                } else {
                    paths.len() - 1
                }
            }
            Some(current) => {
                let cur_idx = paths.iter().position(|p| p == current).unwrap_or(0) as isize;
                (cur_idx + delta).clamp(0, paths.len() as isize - 1) as usize
            }
        };
        self.selected = Some(paths[new_index].clone());
    }

    pub(crate) fn collapse_or_parent(&mut self) {
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let Some(tree) = self.project_tree.as_mut() else {
            return;
        };
        let snapshot = snapshot_row(tree, &selected);
        if let Some((kind, expanded, _depth)) = snapshot {
            if matches!(kind, EntryKind::Directory) && expanded {
                tree.collapse(&selected);
                return;
            }
        }
        // 不是展开目录：上跳到父行。根目录已经是最顶层，原地不动。
        if selected == tree.root() {
            return;
        }
        if let Some(parent) = selected.parent() {
            self.selected = Some(parent.to_path_buf());
        }
    }

    pub(crate) fn expand_or_into(&mut self) {
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let Some(tree) = self.project_tree.as_mut() else {
            return;
        };
        let Some((kind, expanded, depth)) = snapshot_row(tree, &selected) else {
            return;
        };
        if !matches!(kind, EntryKind::Directory) {
            return;
        }
        if expanded {
            let next_child = tree
                .visible_rows()
                .iter()
                .enumerate()
                .find_map(|(idx, row)| {
                    if row.path == selected {
                        Some(idx)
                    } else {
                        None
                    }
                });
            if let Some(idx) = next_child {
                let next = tree
                    .visible_rows()
                    .get(idx + 1)
                    .filter(|row| row.depth > depth)
                    .map(|row| row.path.to_path_buf());
                if let Some(path) = next {
                    self.selected = Some(path);
                }
            }
        } else if let Err(error) = tree.expand(&selected) {
            eprintln!("展开目录失败：{}：{error}", selected.display());
        }
    }

    pub(crate) fn activate_selected(
        &mut self,
        workspace: &mut Workspace,
        views: &mut ViewSet,
    ) -> FileTreeActivation {
        let Some(selected) = self.selected.clone() else {
            return FileTreeActivation::Nothing;
        };
        let Some(tree) = self.project_tree.as_mut() else {
            return FileTreeActivation::Nothing;
        };
        let Some((kind, _, _)) = snapshot_row(tree, &selected) else {
            return FileTreeActivation::Nothing;
        };
        match kind {
            EntryKind::Directory => {
                if let Err(error) = tree.toggle(&selected) {
                    eprintln!("切换目录展开失败：{}：{error}", selected.display());
                }
                FileTreeActivation::ToggledDir
            }
            EntryKind::File => open_file(workspace, views, selected),
        }
    }
}

fn open_file(workspace: &mut Workspace, views: &mut ViewSet, path: PathBuf) -> FileTreeActivation {
    let existing = workspace.buffers().find_map(|(id, buffer)| {
        if buffer.path() == Some(path.as_path()) {
            Some(id)
        } else {
            None
        }
    });
    if let Some(buffer_id) = existing {
        let _ = workspace.set_active_buffer(buffer_id);
        focus_buffer_view(workspace, views, buffer_id);
        return FileTreeActivation::OpenedFile;
    }

    let buffer_id = match workspace.open_file(path.clone()) {
        Ok(buffer_id) => buffer_id,
        Err(error) => {
            eprintln!("打开文件失败：{}：{error}", path.display());
            return FileTreeActivation::Nothing;
        }
    };
    focus_buffer_view(workspace, views, buffer_id);
    FileTreeActivation::OpenedFile
}

/// 确保 `buffer_id` 有对应视图，并把它切成活动视图。
///
/// `ViewSet::open_view` 只在当前无活动视图时才自动激活，而 app 启动即带一个
/// 空 buffer 视图，所以打开新文件后必须显式 `set_active`，否则编辑区仍显示旧
/// buffer。已存在视图时复用，不重复建。
fn focus_buffer_view(workspace: &Workspace, views: &mut ViewSet, buffer_id: BufferId) {
    let existing_view = views.views().find_map(|(id, view)| {
        if view.buffer() == buffer_id {
            Some(id)
        } else {
            None
        }
    });
    let view_id = match existing_view {
        Some(id) => id,
        None => {
            let version = workspace
                .buffer(buffer_id)
                .expect("刚打开的 buffer 必然存在")
                .buffer()
                .version();
            views.open_view(buffer_id, version)
        }
    };
    views.set_active(view_id);
}

/// 从一棵 [`ProjectTree`] 里抓出一行的 `(kind, expanded, depth)`，规避借用
/// 重叠：调用方拿到 owned 元组后即可继续对树做可变操作。
fn snapshot_row(tree: &ProjectTree, path: &Path) -> Option<(EntryKind, bool, usize)> {
    tree.visible_rows()
        .into_iter()
        .find(|row| row.path == path)
        .map(|row| (row.kind, row.expanded, row.depth))
}
