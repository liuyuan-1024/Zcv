//! HistoryEntry 数据边界：描述一次可撤销历史节点所需的 undo/redo 批次与前后 selection。
//!
//! 本文件只保存可重放事实和最小构造逻辑，不管理栈顺序、redo 清理或事务来源策略。
//!
//! **Zero-copy 纪律**：
//! - `undo_batches` / `redo_batches` 用 `Arc<[EditList]>`，每个 `EditList` 内部又是 `Arc<[Edit]>`
//! - `description` 用 `Option<Arc<str>>`
//! - `HistoryEntry::clone()` 全程 O(1) 引用计数递增，`undo()` / `redo()` / `merge_into_current` 不再深拷贝

use std::sync::Arc;

use crate::{SelectionSet, transaction::EditList};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::buffer) struct HistoryEntry {
    pub(in crate::buffer) undo_batches: Arc<[EditList]>,
    pub(in crate::buffer) redo_batches: Arc<[EditList]>,
    pub(in crate::buffer) before_selection: SelectionSet,
    pub(in crate::buffer) after_selection: SelectionSet,
    pub(in crate::buffer) description: Option<Arc<str>>,
}

impl HistoryEntry {
    pub(in crate::buffer) fn new(
        undo_edits: EditList,
        redo_edits: EditList,
        before_selection: SelectionSet,
        after_selection: SelectionSet,
        description: Option<Arc<str>>,
    ) -> Self {
        Self {
            undo_batches: Arc::from(vec![undo_edits]),
            redo_batches: Arc::from(vec![redo_edits]),
            before_selection,
            after_selection,
            description,
        }
    }

    /// `HistoryEntry` 在历史预算中的字节占用估算。
    ///
    /// 度量 = `undo_batches` 与 `redo_batches` 中所有 `Edit::replacement` 的 UTF-8
    /// 字节和；selection / description / TextRange / EditList 容器本身不计入。
    /// 这反映了 Undo 复原所需字符串的实际开销，是引擎能稳定承诺的最小事实。
    pub(in crate::buffer) fn byte_size(&self) -> usize {
        let undo: usize = self
            .undo_batches
            .iter()
            .flat_map(|list| list.as_slice())
            .map(|edit| edit.replacement().len())
            .sum();
        let redo: usize = self
            .redo_batches
            .iter()
            .flat_map(|list| list.as_slice())
            .map(|edit| edit.replacement().len())
            .sum();
        undo + redo
    }

    pub(in crate::buffer) fn merge(previous: Self, next: Self) -> Self {
        // 合并需要新分配 Arc<[T]>：先拼到 Vec 再 Arc::from。
        // Arc 本身的 clone 是 O(1)；EditList 的 clone 也是 O(1)（其内部 Arc<[Edit]>）。
        let mut undo_batches: Vec<EditList> =
            Vec::with_capacity(next.undo_batches.len() + previous.undo_batches.len());
        undo_batches.extend(next.undo_batches.iter().cloned());
        undo_batches.extend(previous.undo_batches.iter().cloned());

        let mut redo_batches: Vec<EditList> =
            Vec::with_capacity(previous.redo_batches.len() + next.redo_batches.len());
        redo_batches.extend(previous.redo_batches.iter().cloned());
        redo_batches.extend(next.redo_batches.iter().cloned());

        let description = next.description.or(previous.description);

        Self {
            undo_batches: Arc::from(undo_batches),
            redo_batches: Arc::from(redo_batches),
            before_selection: previous.before_selection,
            after_selection: next.after_selection,
            description,
        }
    }
}
