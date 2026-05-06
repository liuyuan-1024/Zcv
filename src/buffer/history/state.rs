//! HistoryState 栈管理：维护线性 undo/redo 栈及其摘要状态。
//!
//! 本文件只管理历史容器的局部不变量，不读取 Buffer 文本，也不生成 inverse edits。

use super::HistoryEntry;

/// M3 历史状态摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryStatus {
    pub undo_depth: usize,
    pub redo_depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(in crate::buffer) struct HistoryState {
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
}

impl HistoryState {
    pub(in crate::buffer) fn new() -> Self {
        Self::default()
    }

    pub(in crate::buffer) fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub(in crate::buffer) fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub(in crate::buffer) fn status(&self) -> HistoryStatus {
        HistoryStatus {
            undo_depth: self.undo_stack.len(),
            redo_depth: self.redo_stack.len(),
        }
    }

    pub(in crate::buffer) fn pop_undo(&mut self) -> Option<HistoryEntry> {
        self.undo_stack.pop()
    }

    pub(in crate::buffer) fn pop_redo(&mut self) -> Option<HistoryEntry> {
        self.redo_stack.pop()
    }

    pub(in crate::buffer) fn push_undo(&mut self, entry: HistoryEntry) {
        self.undo_stack.push(entry);
    }

    pub(in crate::buffer) fn push_redo(&mut self, entry: HistoryEntry) {
        self.redo_stack.push(entry);
    }

    pub(in crate::buffer) fn clear_redo(&mut self) {
        self.redo_stack.clear();
    }

    pub(in crate::buffer) fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    pub(in crate::buffer) fn redo_is_empty(&self) -> bool {
        self.redo_stack.is_empty()
    }

    pub(in crate::buffer) fn truncate_undo_to(&mut self, max: usize) {
        if max == 0 {
            self.undo_stack.clear();
            return;
        }

        while self.undo_stack.len() > max {
            self.undo_stack.remove(0);
        }
    }
}
