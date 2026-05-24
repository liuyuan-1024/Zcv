//! 文件树运行模型。
//!
//! 负责项目目录树、展开状态、选中行、面板快照构造，以及从文件树激活文件。

use std::path::{Path, PathBuf};

use zom_command::EditTarget;
use zom_view::{ViewId, ViewSet};
use zom_workspace::{BufferId, EntryKind, ProjectTree, Workspace};

use crate::shell::editor::{
    Editor, EditorSnapshot, ImeQueryTarget, ImeTarget, TextInputProfile, TextTargetId,
    TextTargetOwner, TextTargetQuery,
};

use super::{FileTreeActivation, FileTreeRow, FileTreeState, PendingDelete, PendingNewEntry};

#[derive(Default)]
pub(crate) struct FileTreeModel {
    project_tree: Option<ProjectTree>,
    selected: Option<PathBuf>,
    /// 正在键入名称的新建条目；`None` 表示不处于新建态。
    pending: Option<PendingEntry>,
    /// 正在等待确认的待删条目（路径 + 类型）；`None` 表示无删除确认弹窗。
    pending_delete: Option<(PathBuf, EntryKind)>,
}

/// 新建态的内部数据；缩进深度在 `state()` 快照时再算，故此处不存。
///
/// 名称由一个 [`Editor`] 承载 —— 键入 / 删除 / undo / 选择都复用编辑命令。
struct PendingEntry {
    parent: PathBuf,
    kind: EntryKind,
    editor: Editor,
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
        self.pending = None;
        self.pending_delete = None;
    }

    pub(crate) fn state(&self, workspace: &Workspace) -> FileTreeState {
        let Some(tree) = self.project_tree.as_ref() else {
            return FileTreeState::default();
        };
        let rows: Vec<FileTreeRow> = tree
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
        // 输入行缩进 = 父目录行 depth + 1；父目录必可见（新建前已展开它）。
        let pending = self.pending.as_ref().map(|pending| {
            let depth = rows
                .iter()
                .find(|row| row.path == pending.parent)
                .map(|row| row.depth + 1)
                .unwrap_or(1);
            PendingNewEntry {
                parent: pending.parent.clone(),
                kind: pending.kind,
                depth,
            }
        });
        let pending_delete = self
            .pending_delete
            .as_ref()
            .map(|(path, kind)| PendingDelete {
                name: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                kind: *kind,
            });
        FileTreeState {
            rows,
            selected: self.selected.clone(),
            active,
            pending,
            pending_delete,
        }
    }

    /// 开始新建一个文件 / 目录：确定目标父目录、展开它，并进入输入态。
    ///
    /// 目标父目录：选中目录则用它本身；选中文件则用其父目录；未选中用根。
    pub(crate) fn begin_new_entry(&mut self, kind: EntryKind) {
        let Some(tree) = self.project_tree.as_mut() else {
            return;
        };
        let parent = match self.selected.as_ref() {
            None => tree.root().to_path_buf(),
            Some(selected) => match snapshot_row(tree, selected) {
                Some((EntryKind::Directory, _, _)) => selected.clone(),
                Some((EntryKind::File, _, _)) => selected
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| tree.root().to_path_buf()),
                None => tree.root().to_path_buf(),
            },
        };
        if let Err(error) = tree.expand(&parent) {
            eprintln!("展开目录失败：{}：{error}", parent.display());
            return;
        }
        self.pending = Some(PendingEntry {
            parent,
            kind,
            editor: Editor::new(),
        });
    }

    pub(crate) fn pending_active(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn cancel_new_entry(&mut self) {
        self.pending = None;
    }

    /// 提交新建：名称为空则等同取消；名称含路径分隔符或创建失败时保留
    /// 输入态，供用户改名重试。
    ///
    /// 新建文件成功后立即打开它（返回 [`FileTreeActivation::OpenedFile`]，
    /// 由调用方把焦点切到编辑器）；新建目录无可打开内容，留在文件树。
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
        if name.contains('/') || name.contains('\\') {
            eprintln!("新建条目名不能含路径分隔符：{name}");
            return FileTreeActivation::Nothing;
        }
        let (parent, kind, name) = (pending.parent.clone(), pending.kind, name.to_string());
        let Some(tree) = self.project_tree.as_mut() else {
            self.pending = None;
            return FileTreeActivation::Nothing;
        };
        match tree.create_entry(&parent, &name, kind) {
            Ok(path) => {
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

    pub(crate) fn pending_delete_active(&self) -> bool {
        self.pending_delete.is_some()
    }

    /// 请求删除选中条目：文件或目录皆可，进入删除确认态。项目根代表项目
    /// 本身、不可删；空选中忽略。
    pub(crate) fn request_delete(&mut self) {
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let Some(tree) = self.project_tree.as_ref() else {
            return;
        };
        if selected == tree.root() {
            return;
        }
        if let Some((kind, _, _)) = snapshot_row(tree, &selected) {
            self.pending_delete = Some((selected, kind));
        }
    }

    /// 确认删除：把待删条目移入回收站，选中跳到其父目录，并关闭受影响的
    /// 编辑器视图 —— 删文件关闭该文件，删目录关闭其下全部文件。失败时记录
    /// 日志，确认态一并关闭，由用户决定是否重试。
    pub(crate) fn confirm_delete(&mut self, workspace: &mut Workspace, views: &mut ViewSet) {
        let Some((path, _kind)) = self.pending_delete.take() else {
            return;
        };
        let Some(tree) = self.project_tree.as_mut() else {
            return;
        };
        match tree.delete_entry(&path) {
            Ok(()) => {
                // 选中不能再指向已删除的行，落到父目录。
                self.selected = path.parent().map(Path::to_path_buf);
                close_buffers_under(workspace, views, &path);
            }
            Err(error) => {
                eprintln!("删除失败：{}：{error}", path.display());
            }
        }
    }

    pub(crate) fn cancel_delete(&mut self) {
        self.pending_delete = None;
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
        if let Some((kind, expanded, _depth)) = snapshot
            && matches!(kind, EntryKind::Directory)
            && expanded
        {
            tree.collapse(&selected);
            return;
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

/// 关闭路径落在 `deleted` 之下（含 `deleted` 自身）的全部 buffer 及其视图 ——
/// 删文件即关该文件，删目录即关其下所有已打开文件。
///
/// 文件已不在磁盘上，buffer 一并关闭（不像关标签那样保留）：再留着只是
/// 指向已删文件的孤儿。关完后把 workspace 活动 buffer 对齐到剩余活动视图，
/// 让文件树「活动文件」高亮跟随。
fn close_buffers_under(workspace: &mut Workspace, views: &mut ViewSet, deleted: &Path) {
    let victims: Vec<BufferId> = workspace
        .buffers()
        .filter_map(|(id, buffer)| {
            buffer
                .path()
                .filter(|path| path.starts_with(deleted))
                .map(|_| id)
        })
        .collect();
    for buffer_id in victims {
        let view_ids: Vec<ViewId> = views
            .views()
            .filter_map(|(view_id, view)| (view.buffer() == buffer_id).then_some(view_id))
            .collect();
        for view_id in view_ids {
            views.close_view(view_id);
        }
        let _ = workspace.close_buffer(buffer_id);
    }
    if let Some(buffer_id) = views.active_view().map(|view| view.buffer()) {
        let _ = workspace.set_active_buffer(buffer_id);
    }
}

/// 从一棵 [`ProjectTree`] 里抓出一行的 `(kind, expanded, depth)`，规避借用
/// 重叠：调用方拿到 owned 元组后即可继续对树做可变操作。
fn snapshot_row(tree: &ProjectTree, path: &Path) -> Option<(EntryKind, bool, usize)> {
    tree.visible_rows()
        .into_iter()
        .find(|row| row.path == path)
        .map(|row| (row.kind, row.expanded, row.depth))
}

impl TextTargetQuery for FileTreeModel {
    fn target_id(&self) -> TextTargetId {
        TextTargetId::FileTreePendingName
    }

    fn is_active(&self) -> bool {
        self.pending.is_some()
    }

    fn snapshot(&self) -> EditorSnapshot {
        self.pending
            .as_ref()
            .map(|pending| pending.editor.snapshot())
            .unwrap_or_default()
    }

    fn profile(&self) -> TextInputProfile {
        TextInputProfile::FileTreePendingName
    }

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        self.pending
            .as_ref()
            .map(|pending| pending.editor.as_ime_query_target())
    }
}

impl TextTargetOwner for FileTreeModel {
    fn ime_target(&mut self) -> Option<ImeTarget<'_>> {
        self.pending
            .as_mut()
            .map(|pending| pending.editor.as_ime_target())
    }

    fn edit_target(&mut self) -> Option<EditTarget<'_>> {
        self.pending
            .as_mut()
            .map(|pending| pending.editor.as_edit_target())
    }
}
