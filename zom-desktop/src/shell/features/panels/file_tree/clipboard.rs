//! 文件树内部剪贴板。

use std::path::PathBuf;

use super::model::FileTreeModel;

/// 内部剪贴板的两种模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardMode {
    Copy,
    Cut,
}

/// 一次 copy / cut 拍下的路径集合与模式快照。
#[derive(Clone, Debug)]
pub(super) struct FileTreeClipboard {
    pub(super) mode: ClipboardMode,
    pub(super) paths: Vec<PathBuf>,
}

impl FileTreeModel {
    /// 复制：把当前选区（空时降级到焦点单项）拍进内部剪贴板，模式 Copy。
    pub(crate) fn copy_to_clipboard(&mut self) {
        let paths = self.clipboard_snapshot_source();
        if paths.is_empty() {
            return;
        }
        self.clipboard = Some(FileTreeClipboard {
            mode: ClipboardMode::Copy,
            paths,
        });
    }

    /// 剪切：与 [`copy_to_clipboard`](Self::copy_to_clipboard) 同理但模式 Cut。
    pub(crate) fn cut_to_clipboard(&mut self) {
        let paths = self.clipboard_snapshot_source();
        if paths.is_empty() {
            return;
        }
        self.clipboard = Some(FileTreeClipboard {
            mode: ClipboardMode::Cut,
            paths,
        });
    }

    /// 拍剪贴板源：合并视图（已提交选区 + 当前笔画）非空用合并视图；否则降级
    /// 到焦点单项；都没有则空。
    fn clipboard_snapshot_source(&self) -> Vec<PathBuf> {
        let effective = self.effective_selection();
        if !effective.is_empty() {
            effective.into_iter().collect()
        } else if let Some(focus) = self.selected.clone() {
            vec![focus]
        } else {
            Vec::new()
        }
    }
}
