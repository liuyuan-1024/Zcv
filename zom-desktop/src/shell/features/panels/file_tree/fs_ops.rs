//! 文件树文件系统操作与目录树操作。

use std::io;
use std::path::{Path, PathBuf};

use zom_view::ViewSet;
use zom_workspace::{EntryKind, ProjectTree, Workspace};

use super::FileTreeActivation;
use super::clipboard::ClipboardMode;
use super::model::FileTreeModel;
use super::workspace_sync::{
    close_buffers_under, open_file, rebase_buffers_under, selected_path_after_deleting_active,
};

impl FileTreeModel {
    /// 提交新建：名称为空则等同取消；以 `/` 结尾创建目录，否则创建文件。
    pub(crate) fn commit_new_entry(
        &mut self,
        workspace: &mut Workspace,
        views: &mut ViewSet,
    ) -> FileTreeActivation {
        let Some(pending) = self.pending.as_ref() else {
            return FileTreeActivation::Nothing;
        };
        let text = pending.editor.text();
        let name = text.trim();
        if name.is_empty() {
            self.pending = None;
            return FileTreeActivation::Nothing;
        }
        let Some((name, kind)) = parse_new_entry_input(name) else {
            eprintln!("新建条目路径无效：{name}");
            return FileTreeActivation::Nothing;
        };
        let parent = pending.parent.clone();
        let Some(tree) = self.project_tree.as_mut() else {
            self.pending = None;
            return FileTreeActivation::Nothing;
        };
        match tree.create_entry(&parent, &name, kind) {
            Ok(path) => {
                expand_parent_chain(tree, &parent, &path);
                self.selected = Some(path.clone());
                self.pending = None;
                match kind {
                    EntryKind::File => open_file(workspace, views, path),
                    EntryKind::Directory => FileTreeActivation::Nothing,
                }
            }
            Err(error) => {
                let label = match kind {
                    EntryKind::Directory => "目录",
                    EntryKind::File => "文件",
                };
                eprintln!("新建{label}失败：{}：{error}", parent.join(&name).display());
                FileTreeActivation::Nothing
            }
        }
    }

    /// 提交重命名：把当前输入框文本作为新名落盘。
    pub(crate) fn commit_rename(
        &mut self,
        workspace: &mut Workspace,
        views: &mut ViewSet,
    ) -> FileTreeActivation {
        let Some(pending) = self.pending_rename.as_ref() else {
            return FileTreeActivation::Nothing;
        };
        let text = pending.editor.text();
        let new_name = text.trim();
        if new_name.is_empty() {
            return FileTreeActivation::Nothing;
        }
        if pending
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == new_name)
            .unwrap_or(false)
        {
            self.pending_rename = None;
            return FileTreeActivation::Nothing;
        }
        let old_path = pending.path.clone();
        let Some(tree) = self.project_tree.as_mut() else {
            self.pending_rename = None;
            return FileTreeActivation::Nothing;
        };
        match tree.rename_entry(&old_path, new_name) {
            Ok(new_path) => {
                let is_file = new_path.is_file();
                if !is_file && let Err(error) = tree.expand(&new_path) {
                    eprintln!("重命名后展开目录失败：{}：{error}", new_path.display());
                }
                let next_focus = if is_file {
                    new_path.clone()
                } else {
                    neighbor_after(tree, &new_path).unwrap_or_else(|| new_path.clone())
                };
                rebase_buffers_under(workspace, &old_path, &new_path);
                self.selected = Some(next_focus);
                self.pending_rename = None;
                if is_file {
                    open_file(workspace, views, new_path)
                } else {
                    FileTreeActivation::Nothing
                }
            }
            Err(error) => {
                eprintln!("重命名失败：{} → {new_name}：{error}", old_path.display());
                FileTreeActivation::Nothing
            }
        }
    }

    /// 请求删除：选区非空时把整个选区拍进 pending_delete；选区为空时降级到
    /// 焦点单项。项目根不可删。
    pub(crate) fn request_delete(&mut self) {
        let Some(tree) = self.project_tree.as_ref() else {
            return;
        };
        let effective = self.effective_selection();
        let candidates: Vec<PathBuf> = if !effective.is_empty() {
            effective.into_iter().collect()
        } else if let Some(focus) = self.selected.clone() {
            vec![focus]
        } else {
            return;
        };
        let root = tree.root().to_path_buf();
        let mut items: Vec<(PathBuf, EntryKind)> = Vec::new();
        for path in candidates {
            if path == root {
                continue;
            }
            if let Some((kind, _, _)) = snapshot_row(tree, &path) {
                items.push((path, kind));
            }
        }
        if items.is_empty() {
            return;
        }
        self.pending_delete = Some(items);
    }

    /// 确认删除：把待删集合里的每一项移入回收站、关闭受影响的编辑器视图。
    pub(crate) fn confirm_delete(&mut self, workspace: &mut Workspace, views: &mut ViewSet) {
        let Some(items) = self.pending_delete.take() else {
            return;
        };
        let Some(tree) = self.project_tree.as_mut() else {
            return;
        };
        let next_sibling = items.first().and_then(|(first_path, _)| {
            next_sibling_of(tree, first_path, |candidate| {
                items
                    .iter()
                    .any(|(deleted, _)| deleted.as_path() == candidate)
            })
        });
        for (path, _) in &items {
            match delete_tree_entry(tree, path) {
                Ok(()) => close_buffers_under(workspace, views, path),
                Err(error) => {
                    eprintln!("删除失败：{}：{error}", path.display());
                }
            }
        }
        self.selected =
            next_sibling.or_else(|| selected_path_after_deleting_active(tree, workspace));
        self.selection.clear();
        self.stroke = None;
    }

    pub(crate) fn cancel_delete(&mut self) {
        self.pending_delete = None;
    }

    /// 粘贴：把剪贴板内容应用到“焦点所在目录”。
    pub(crate) fn paste_from_clipboard(&mut self, workspace: &mut Workspace) {
        let Some(clipboard) = self.clipboard.clone() else {
            return;
        };
        let target_parent = {
            let Some(tree) = self.project_tree.as_ref() else {
                return;
            };
            compute_paste_target(tree, self.selected.as_ref())
        };
        let Some(tree) = self.project_tree.as_mut() else {
            return;
        };
        let mut new_paths = Vec::new();
        for src in &clipboard.paths {
            let result = match clipboard.mode {
                ClipboardMode::Copy => tree.copy_entry(src, &target_parent),
                ClipboardMode::Cut => tree.move_entry(src, &target_parent),
            };
            match result {
                Ok(new_path) => {
                    if matches!(clipboard.mode, ClipboardMode::Cut) && new_path != *src {
                        rebase_buffers_under(workspace, src, &new_path);
                    }
                    new_paths.push(new_path);
                }
                Err(error) => {
                    eprintln!(
                        "粘贴失败：{} → {}：{error}",
                        src.display(),
                        target_parent.display()
                    );
                }
            }
        }
        self.selection.clear();
        self.stroke = None;
        if matches!(clipboard.mode, ClipboardMode::Cut) {
            self.clipboard = None;
        }
        if !new_paths.is_empty() {
            if let Err(error) = tree.expand(&target_parent) {
                eprintln!("粘贴后展开目录失败：{}：{error}", target_parent.display());
            }
            self.selected = Some(target_parent);
        }
    }

    pub(crate) fn collapse_or_parent(&mut self) {
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let Some(tree) = self.project_tree.as_mut() else {
            return;
        };
        let snapshot = snapshot_row(tree, &selected);
        if let Some((kind, expanded, _depth)) = snapshot
            && matches!(kind, EntryKind::Directory)
            && expanded
        {
            tree.collapse(&selected);
            return;
        }
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

pub(super) fn infer_entry_kind_from_input(text: &str) -> EntryKind {
    match parse_new_entry_input(text.trim()) {
        Some((_, kind)) => kind,
        None => EntryKind::File,
    }
}

fn parse_new_entry_input(input: &str) -> Option<(String, EntryKind)> {
    if input.is_empty() || input.contains('\\') {
        return None;
    }
    let kind = if input.ends_with('/') {
        EntryKind::Directory
    } else {
        EntryKind::File
    };
    let path = input.trim_end_matches('/');
    if path.is_empty()
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return None;
    }
    Some((path.to_string(), kind))
}

fn expand_parent_chain(tree: &mut ProjectTree, base: &Path, path: &Path) {
    let Some(target_parent) = path.parent() else {
        return;
    };
    let mut dirs = Vec::new();
    let mut current = Some(target_parent);
    while let Some(dir) = current {
        if !dir.starts_with(base) {
            break;
        }
        dirs.push(dir.to_path_buf());
        if dir == base {
            break;
        }
        current = dir.parent();
    }
    dirs.reverse();
    for dir in dirs {
        if let Err(error) = tree.expand(&dir) {
            eprintln!("展开目录失败：{}：{error}", dir.display());
            break;
        }
    }
}

#[cfg(not(test))]
fn delete_tree_entry(tree: &mut ProjectTree, path: &Path) -> io::Result<()> {
    tree.delete_entry(path)
}

#[cfg(test)]
fn delete_tree_entry(tree: &mut ProjectTree, path: &Path) -> io::Result<()> {
    tree.delete_entry_permanently(path)
}

fn neighbor_after(tree: &ProjectTree, path: &Path) -> Option<PathBuf> {
    let rows = tree.visible_rows();
    let idx = rows.iter().position(|row| row.path == path)?;
    rows.get(idx + 1)
        .or_else(|| idx.checked_sub(1).and_then(|prev| rows.get(prev)))
        .map(|row| row.path.to_path_buf())
}

pub(super) fn next_sibling_of(
    tree: &ProjectTree,
    path: &Path,
    skip: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let parent = path.parent()?;
    let rows = tree.visible_rows();
    let idx = rows.iter().position(|row| row.path == path)?;
    rows.iter().skip(idx + 1).find_map(|row| {
        if row.path.parent() != Some(parent) {
            return None;
        }
        if skip(row.path) {
            return None;
        }
        Some(row.path.to_path_buf())
    })
}

/// 从一棵 [`ProjectTree`] 里抓出一行的 `(kind, expanded, depth)`，规避借用重叠。
pub(super) fn snapshot_row(tree: &ProjectTree, path: &Path) -> Option<(EntryKind, bool, usize)> {
    tree.visible_rows()
        .into_iter()
        .find(|row| row.path == path)
        .map(|row| (row.kind, row.expanded, row.depth))
}

pub(super) fn compute_paste_target(tree: &ProjectTree, selected: Option<&PathBuf>) -> PathBuf {
    match selected {
        None => tree.root().to_path_buf(),
        Some(path) => match snapshot_row(tree, path) {
            Some((EntryKind::Directory, _, _)) => path.clone(),
            Some((EntryKind::File, _, _)) => path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| tree.root().to_path_buf()),
            None => tree.root().to_path_buf(),
        },
    }
}
