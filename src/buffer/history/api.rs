//! History public API：把 Undo / Redo 能力暴露为 Buffer 方法，并维护 selection 恢复语义。
//!
//! 本文件负责历史栈出入栈和重放入口，不定义 HistoryEntry 的存储形态，也不参与普通事务准备。

use crate::{
    CharOffset, EngineResult, TransactionSource,
    buffer::Buffer,
    transaction::{ChangeSet, Delta, Edit, EditList, TransactionMergePolicy, TransactionMetadata},
};

use super::{HistoryEntry, HistoryStatus};

impl Buffer {
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn history_status(&self) -> HistoryStatus {
        self.history.status()
    }

    /// 撤销最近一次历史节点。
    ///
    /// 没有可撤销历史时返回 `Ok(None)`，避免把空历史当作错误。
    pub fn undo(&mut self) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.ensure_writable()?;
        self.cancel_composition_before_text_edit()?;

        let Some(entry) = self.history.pop_undo() else {
            return Ok(None);
        };

        let mut result = None;
        for tx_edits in &entry.undo_batches {
            result = Some(self.apply_edit_list(
                self.version,
                tx_edits.clone(),
                TransactionSource::Undo,
            )?);
        }

        let result = result.expect("history entry must contain at least one undo batch");
        self.selection = entry.before_selection.clone();
        self.history.push_redo(entry);

        Ok(Some(result))
    }

    /// 重做最近一次被撤销的历史节点。
    ///
    /// 没有可重做历史时返回 `Ok(None)`。
    pub fn redo(&mut self) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.ensure_writable()?;
        self.cancel_composition_before_text_edit()?;

        let Some(entry) = self.history.pop_redo() else {
            return Ok(None);
        };

        let mut result = None;
        for tx_edits in &entry.redo_batches {
            result = Some(self.apply_edit_list(
                self.version,
                tx_edits.clone(),
                TransactionSource::Redo,
            )?);
        }

        let result = result.expect("history entry must contain at least one redo batch");
        self.selection = entry.after_selection.clone();
        self.history.push_undo(entry);

        Ok(Some(result))
    }

    pub(in crate::buffer) fn push_history(
        &mut self,
        entry: HistoryEntry,
        metadata: &TransactionMetadata,
    ) -> EngineResult<()> {
        if metadata.merge_policy == TransactionMergePolicy::MergeWithPrevious
            && self.history.redo_is_empty()
        {
            if let Some(previous) = self.history.pop_undo() {
                let merged = HistoryEntry::merge(previous, entry);
                self.history.push_undo(merged);
                self.truncate_undo_history_to_budget();
                return Ok(());
            }
        }

        self.history.push_undo(entry);
        self.history.clear_redo();
        self.truncate_undo_history_to_budget();

        Ok(())
    }

    fn truncate_undo_history_to_budget(&mut self) {
        self.history
            .truncate_undo_to(self.config.large_file.max_undo_history);
    }

    pub(in crate::buffer) fn build_inverse_edit_list(
        &self,
        edits: &EditList,
    ) -> EngineResult<EditList> {
        let mut inverse = Vec::with_capacity(edits.len());
        let mut diff = 0isize;

        for edit in edits.as_slice() {
            let old_start = edit.range.start().get();
            let old_end = edit.range.end().get();
            let deleted_text = self.slice_text(edit.range)?.to_string();

            let new_start = (old_start as isize + diff).max(0) as usize;
            let new_end = new_start + edit.replacement.chars().count();
            let new_range =
                crate::TextRange::new(CharOffset::new(new_start), CharOffset::new(new_end))?;

            inverse.push(Edit::replace(new_range, deleted_text));

            diff += edit.replacement.chars().count() as isize - (old_end - old_start) as isize;
        }

        Ok(EditList::new(inverse)?)
    }
}
