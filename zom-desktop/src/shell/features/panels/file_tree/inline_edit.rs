//! 文件树内联输入框与文本目标适配。

use std::path::{Path, PathBuf};

use zom_command::commands::file_tree::FileTreeKeyMode;
use zom_command::{BubbleRequest, EditTarget, KeyContext};

use crate::editor::text::{
    EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, OwnedEditorTarget,
};
use crate::focus::{AppFocus, FileTreeFocus};
use crate::text_target::{TextTargetOwner, TextTargetQuery};

use super::fs_ops::snapshot_row;
use super::model::FileTreeModel;
use zom_workspace::EntryKind;

/// 新建态的内部数据；缩进深度在 `state()` 快照时再算，故此处不存。
pub(super) struct PendingEntry {
    pub(super) parent: PathBuf,
    pub(super) editor: OwnedEditorTarget,
}

/// 重命名态的内部数据。
/// `path` 是目标条目（旧路径），
/// 输入框由 [`OwnedEditorTarget`] 承载并在 `begin_rename` 处预填旧名 + 光标在末尾。
pub(super) struct PendingRenameEntry {
    pub(super) path: PathBuf,
    pub(super) editor: OwnedEditorTarget,
}

impl FileTreeModel {
    /// 开始新建一个文件或目录：确定目标父目录、展开它，并进入输入态。
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
            self.pending_bubbles.push(
                BubbleRequest::error(format!("展开目录失败：{}：{error}", parent.display()))
                    .dedupe("file_tree.expand"),
            );
            return;
        }
        self.pending = Some(PendingEntry {
            parent,
            editor: OwnedEditorTarget::new(),
        });
    }

    pub(crate) fn cancel_new_entry(&mut self) {
        self.pending = None;
    }

    /// 进入重命名态：以当前焦点行为目标，输入框预填旧名并把光标放到末尾。
    pub(crate) fn begin_rename(&mut self) {
        if self.pending.is_some() || self.pending_delete.is_some() {
            return;
        }
        let Some(tree) = self.project_tree.as_ref() else {
            return;
        };
        let Some(path) = self.selected.clone() else {
            return;
        };
        if path == tree.root() {
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.is_empty() {
            return;
        }
        self.pending_rename = Some(PendingRenameEntry {
            path,
            editor: OwnedEditorTarget::with_text_caret_at_end(&name),
        });
    }

    pub(crate) fn cancel_rename(&mut self) {
        self.pending_rename = None;
    }
}

impl TextTargetQuery for FileTreeModel {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        let sub = match focus {
            AppFocus::Panel(p) => p.as_file_tree(),
            _ => None,
        };
        match sub {
            Some(FileTreeFocus::NewEntryName) => self.pending.is_some(),
            Some(FileTreeFocus::RenameEntry) => self.pending_rename.is_some(),
            _ => false,
        }
    }

    fn snapshot(&self, _focus: AppFocus) -> EditorSnapshot {
        if let Some(rename) = self.pending_rename.as_ref() {
            return rename.editor.snapshot(EditorSnapshotRequest::single_line());
        }
        self.pending
            .as_ref()
            .map(|pending| {
                pending
                    .editor
                    .snapshot(EditorSnapshotRequest::single_line())
            })
            .unwrap_or_default()
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        let file_tree_mode = if self.pending_rename.is_some() {
            FileTreeKeyMode::PendingRename
        } else {
            FileTreeKeyMode::PendingName
        };
        vec![
            KeyContext::file_tree(file_tree_mode),
            KeyContext::text_edit(self.accepts_newline(), false),
            KeyContext::global(),
        ]
    }

    fn ime_query_target(&self, _focus: AppFocus) -> Option<ImeQueryTarget<'_>> {
        if let Some(rename) = self.pending_rename.as_ref() {
            return Some(rename.editor.as_ime_query_target());
        }
        self.pending
            .as_ref()
            .map(|pending| pending.editor.as_ime_query_target())
    }
}

impl TextTargetOwner for FileTreeModel {
    fn edit_target(&mut self, _focus: AppFocus) -> Option<EditTarget<'_>> {
        if let Some(rename) = self.pending_rename.as_mut() {
            return Some(rename.editor.as_edit_target());
        }
        self.pending
            .as_mut()
            .map(|pending| pending.editor.as_edit_target())
    }
}
