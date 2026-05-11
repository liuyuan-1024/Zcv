//! HistoryEntry 数据边界：描述一次可撤销历史节点所需的 undo/redo 批次与前后 selection。
//!
//! 本文件只保存可重放事实和最小构造逻辑，不管理栈顺序、redo 清理或事务来源策略。

use crate::{
    ByteOffset, EngineResult, SelectionSet, TextRange,
    transaction::{Edit, EditList},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::buffer) struct HistoryEntry {
    pub(in crate::buffer) undo_batches: Vec<EditList>,
    pub(in crate::buffer) redo_batches: Vec<EditList>,
    pub(in crate::buffer) before_selection: SelectionSet,
    pub(in crate::buffer) after_selection: SelectionSet,
    pub(in crate::buffer) description: Option<String>,
}

impl HistoryEntry {
    pub(in crate::buffer) fn new(
        undo_edits: EditList,
        redo_edits: EditList,
        before_selection: SelectionSet,
        after_selection: SelectionSet,
        description: Option<String>,
    ) -> Self {
        Self {
            undo_batches: vec![undo_edits],
            redo_batches: vec![redo_edits],
            before_selection,
            after_selection,
            description,
        }
    }

    pub(in crate::buffer) fn from_snapshots(
        before_text: String,
        after_text: String,
        before_selection: SelectionSet,
        after_selection: SelectionSet,
        description: Option<String>,
    ) -> EngineResult<Self> {
        let before_range = TextRange::new(
            ByteOffset::ZERO,
            ByteOffset::new(before_text.len()),
        )?;
        let after_range = TextRange::new(
            ByteOffset::ZERO,
            ByteOffset::new(after_text.len()),
        )?;

        let redo_edits = EditList::new(vec![Edit::replace(before_range, after_text.clone())])?;
        let undo_edits = EditList::new(vec![Edit::replace(after_range, before_text.clone())])?;

        Ok(Self::new(
            undo_edits,
            redo_edits,
            before_selection,
            after_selection,
            description,
        ))
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
        let mut undo_batches = next.undo_batches;
        undo_batches.extend(previous.undo_batches);

        let mut redo_batches = previous.redo_batches;
        redo_batches.extend(next.redo_batches);

        let description = next.description.or(previous.description);

        Self {
            undo_batches,
            redo_batches,
            before_selection: previous.before_selection,
            after_selection: next.after_selection,
            description,
        }
    }
}
