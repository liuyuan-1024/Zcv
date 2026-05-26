//! 文件树运行模型。
//!
//! 负责项目目录树、展开状态、选中行、面板快照构造，以及从文件树激活文件。

use std::collections::BTreeSet;
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
    /// 多选集合，独立于 `selected` 焦点。Phase 1 仅承载数据 + 视觉，写入
    /// 入口在后续阶段（Shift+方向键扩选、复制 / 剪切 / 粘贴）补齐。
    selection: BTreeSet<PathBuf>,
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
        self.selection.clear();
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
                kind: infer_entry_kind_from_input(&pending.editor.text()),
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
            selection: self.selection.clone(),
            active,
            pending,
            pending_delete,
        }
    }

    /// 开始新建一个文件或目录：确定目标父目录、展开它，并进入输入态。
    ///
    /// 目标父目录：选中目录则用它本身；选中文件则用其父目录；未选中用根。
    pub(crate) fn begin_new_entry(&mut self) {
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
            editor: Editor::new(),
        });
    }

    pub(crate) fn pending_active(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn cancel_new_entry(&mut self) {
        self.pending = None;
    }

    /// 提交新建：名称为空则等同取消；以 `/` 结尾创建目录，否则创建文件。
    /// 输入中可以包含相对路径，创建时会补齐中间目录；非法路径或创建失败时保留输入态，供用户改名重试。
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
        let deleting_active = workspace
            .active_buffer()
            .and_then(|buffer| buffer.path())
            .is_some_and(|active| active.starts_with(&path));
        match tree.delete_entry(&path) {
            Ok(()) => {
                close_buffers_under(workspace, views, &path);
                if deleting_active {
                    self.selected = selected_path_after_deleting_active(tree, workspace);
                } else {
                    // 选中不能再指向已删除的行，落到父目录。
                    self.selected = path.parent().map(Path::to_path_buf);
                }
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

    /// 扩展多选选区。规则三步：
    /// 1. 当前焦点行加入选区；
    /// 2. 焦点按 `delta` 移动（边界 clamp）；
    /// 3. 新焦点行也加入选区。
    ///
    /// 这样保证从"空选区 + 任意焦点"起按一次 Shift+↓，两端都进选区；后续连续
    /// 按相当于一把"刷子"延伸；中途用方向键跳走再 Shift+↓ 即得到非连续累加。
    /// 不提供"从选区里精确去掉一项"的能力——按设计要求，用 Esc 清空重选。
    pub(crate) fn extend_selection(&mut self, delta: isize) {
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
                // 没有焦点：把目的地（首/末）作为唯一加入项；起点不存在，跳过步骤 1。
                if delta >= 0 { 0 } else { paths.len() - 1 }
            }
            Some(current) => {
                // 步骤 1：起点入选区（即便 current 已不在 paths 里也保留——可能被外部刷新）。
                self.selection.insert(current.clone());
                let cur_idx = paths.iter().position(|p| p == current).unwrap_or(0) as isize;
                (cur_idx + delta).clamp(0, paths.len() as isize - 1) as usize
            }
        };
        // 步骤 2：移动焦点。
        self.selected = Some(paths[new_index].clone());
        // 步骤 3：终点入选区。
        self.selection.insert(paths[new_index].clone());
    }

    /// Esc 二段式：选区非空时清空选区（返回 `true`，表示已消化）；否则返回
    /// `false`，让调用方走"焦点回编辑器"的原有路径。
    pub(crate) fn escape(&mut self) -> bool {
        if self.selection.is_empty() {
            false
        } else {
            self.selection.clear();
            true
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

fn infer_entry_kind_from_input(text: &str) -> EntryKind {
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

fn first_visible_path(tree: &ProjectTree) -> Option<PathBuf> {
    tree.visible_rows()
        .first()
        .map(|row| row.path.to_path_buf())
}

fn selected_path_after_deleting_active(
    tree: &ProjectTree,
    workspace: &Workspace,
) -> Option<PathBuf> {
    workspace
        .active_buffer()
        .and_then(|buffer| buffer.path())
        .map(Path::to_path_buf)
        .or_else(|| first_visible_path(tree))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, create_dir_all};

    fn tmp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zom-file-tree-model-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_dir_all(&dir).unwrap();
        dir
    }

    fn open_view_for(workspace: &Workspace, views: &mut ViewSet, buffer_id: BufferId) -> ViewId {
        let version = workspace
            .buffer(buffer_id)
            .expect("buffer should exist")
            .buffer()
            .version();
        views.open_view(buffer_id, version)
    }

    #[test]
    fn deleting_active_file_should_select_next_active_file_path() {
        let root = tmp_root("delete-active-selects-next");
        let readme = root.join("README.md");
        let lib = root.join("src/lib.rs");
        create_dir_all(root.join("src")).unwrap();
        File::create(&readme).unwrap();
        File::create(&lib).unwrap();

        let tree = ProjectTree::new(root).unwrap();
        let mut workspace = Workspace::new();
        let readme_id = workspace.open_file(readme.clone()).unwrap();
        let lib_id = workspace.open_file(lib.clone()).unwrap();
        let mut views = ViewSet::new();
        open_view_for(&workspace, &mut views, readme_id);
        let lib_view = open_view_for(&workspace, &mut views, lib_id);
        views.set_active(lib_view);

        close_buffers_under(&mut workspace, &mut views, &lib);

        assert_eq!(
            workspace.active_buffer().and_then(|buffer| buffer.path()),
            Some(readme.as_path())
        );
        assert_eq!(
            selected_path_after_deleting_active(&tree, &workspace).as_deref(),
            Some(readme.as_path())
        );
    }

    /// 构造一棵 root/{a,b,c}.txt 的 model，已展开 root。
    fn model_with_three_files(name: &str) -> (FileTreeModel, PathBuf) {
        let root = tmp_root(name);
        File::create(root.join("a.txt")).unwrap();
        File::create(root.join("b.txt")).unwrap();
        File::create(root.join("c.txt")).unwrap();
        let mut model = FileTreeModel::default();
        model.open_project(root.clone());
        (model, root)
    }

    fn workspace_for_state() -> Workspace {
        Workspace::new()
    }

    #[test]
    fn extend_selection_should_add_both_endpoints_first_call() {
        let (mut model, root) = model_with_three_files("extend-first-call");
        // 焦点初始化到第一行（root 自身在 visible_rows 中是第一行）。
        model.ensure_selection_initialized();
        assert_eq!(model.selected.as_deref(), Some(root.as_path()));

        // 第一次 Shift+↓：起点（root）与终点（a.txt）都应进选区。
        model.extend_selection(1);
        let ws = workspace_for_state();
        let state = model.state(&ws);
        assert!(state.selection.contains(&root));
        assert!(state.selection.contains(&root.join("a.txt")));
        assert_eq!(
            state.selected.as_deref(),
            Some(root.join("a.txt").as_path())
        );
    }

    #[test]
    fn extend_selection_continuous_should_grow_like_brush() {
        let (mut model, root) = model_with_three_files("extend-continuous");
        model.ensure_selection_initialized();
        model.extend_selection(1); // root, a
        model.extend_selection(1); // + b
        model.extend_selection(1); // + c
        let state = model.state(&workspace_for_state());
        assert_eq!(state.selection.len(), 4);
        for name in ["a.txt", "b.txt", "c.txt"] {
            assert!(state.selection.contains(&root.join(name)));
        }
        assert!(state.selection.contains(&root));
        assert_eq!(
            state.selected.as_deref(),
            Some(root.join("c.txt").as_path())
        );
    }

    #[test]
    fn plain_move_should_not_touch_selection() {
        let (mut model, root) = model_with_three_files("plain-move-keeps-selection");
        model.ensure_selection_initialized();
        model.extend_selection(1); // 选区 = {root, a}
        // 普通方向键：焦点继续往下，但选区不变。
        model.move_selection(1); // 焦点到 b
        model.move_selection(1); // 焦点到 c
        let state = model.state(&workspace_for_state());
        assert_eq!(state.selection.len(), 2);
        assert!(state.selection.contains(&root));
        assert!(state.selection.contains(&root.join("a.txt")));
        assert_eq!(
            state.selected.as_deref(),
            Some(root.join("c.txt").as_path())
        );
    }

    #[test]
    fn extend_selection_after_jumping_should_accumulate_noncontiguous() {
        let (mut model, root) = model_with_three_files("extend-noncontiguous");
        model.ensure_selection_initialized();
        // 累加前两行入选区。
        model.extend_selection(1); // 选区 = {root, a}
        // 普通方向键跳到 c.txt（中间不留痕迹）。
        model.move_selection(1); // 焦点 b
        model.move_selection(1); // 焦点 c
        // Shift+↑ 在 c 处累加：起点 c 入选区，终点 b 也入选区。
        model.extend_selection(-1);
        let state = model.state(&workspace_for_state());
        assert!(state.selection.contains(&root));
        assert!(state.selection.contains(&root.join("a.txt")));
        assert!(state.selection.contains(&root.join("b.txt")));
        assert!(state.selection.contains(&root.join("c.txt")));
        assert_eq!(state.selection.len(), 4);
        // 起点跳跃留下的"中间区"在这棵小树里恰好没空隙；用更大树测才能完美演示
        // 非连续，这里至少验证 jump 后累加并未"补齐中间所有路径"——root 与 a 是因为
        // 之前累加过、b/c 是这次累加，不存在"多余的中间填充"逻辑。
    }

    #[test]
    fn escape_should_consume_only_when_selection_non_empty() {
        let (mut model, _root) = model_with_three_files("escape-two-stage");
        model.ensure_selection_initialized();
        // 选区空：Esc 不消化。
        assert!(!model.escape());
        // 累加后选区非空：Esc 清空并消化。
        model.extend_selection(1);
        assert!(!model.selection.is_empty());
        assert!(model.escape());
        assert!(model.selection.is_empty());
        // 再次 Esc：又回到空选区不消化。
        assert!(!model.escape());
    }

    #[test]
    fn open_project_should_clear_selection() {
        let (mut model, _root) = model_with_three_files("open-clears-selection");
        model.ensure_selection_initialized();
        model.extend_selection(1);
        assert!(!model.selection.is_empty());
        let other = tmp_root("open-clears-selection-target");
        File::create(other.join("x.txt")).unwrap();
        model.open_project(other);
        assert!(model.selection.is_empty());
    }

    #[test]
    fn deleting_only_active_file_should_select_first_file_tree_row() {
        let root = tmp_root("delete-active-selects-root");
        let readme = root.join("README.md");
        File::create(&readme).unwrap();

        let tree = ProjectTree::new(root.clone()).unwrap();
        let mut workspace = Workspace::new();
        let readme_id = workspace.open_file(readme.clone()).unwrap();
        let mut views = ViewSet::new();
        open_view_for(&workspace, &mut views, readme_id);

        close_buffers_under(&mut workspace, &mut views, &readme);

        assert!(workspace.active_buffer().is_none());
        assert_eq!(
            selected_path_after_deleting_active(&tree, &workspace).as_deref(),
            Some(root.as_path())
        );
    }
}
