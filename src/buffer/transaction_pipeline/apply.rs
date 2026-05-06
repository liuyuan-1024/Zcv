//! 事务应用管线：从 Transaction 校验、准备、提交到 selection 映射和 history 收尾的一站式执行路径。
//!
//! 本文件守住失败原子性和版本推进边界；EditList 归一化、存储实现和 public 便利编辑入口不在这里定义。

use crate::{
    BufferVersion, EngineResult, SelectionSet,
    storage::TextStorage,
    transaction::{ChangeSet, Delta, EditList, Transaction, TransactionSource},
};

use crate::buffer::{Buffer, history::HistoryEntry};

use super::prepared::PreparedTransaction;

impl Buffer {
    /// 提交并应用事务。
    ///
    /// 成功将返回增量事件 Delta 和位置映射器 ChangeSet，并记录 Undo 历史。
    pub fn apply_transaction(&mut self, tx: Transaction) -> EngineResult<(Delta, ChangeSet)> {
        self.ensure_writable()?;
        let prepared = self.prepare_transaction(tx)?;
        let (delta, changeset) = self.commit_prepared_transaction(&prepared)?;
        let after_selection = self.resolve_after_selection(
            &prepared.before_selection,
            prepared.explicit_after_selection.as_ref(),
            &changeset,
        );
        self.selection = after_selection.clone();
        self.finish_transaction(prepared, after_selection)?;

        Ok((delta, changeset))
    }

    fn prepare_transaction(&mut self, tx: Transaction) -> EngineResult<PreparedTransaction> {
        self.cancel_composition_for_transaction(tx.metadata().source)?;

        let (base_version, edits, metadata, before_selection, explicit_after_selection) =
            tx.into_parts();

        self.verify_transaction_base_version(base_version)?;
        self.validate_edit_list(&edits)?;

        let before_selection = before_selection.unwrap_or_else(|| self.selection.clone());
        let undo_edits = self.build_inverse_edit_list(&edits)?;
        let redo_edits = edits.clone();

        Ok(PreparedTransaction {
            base_version,
            edits,
            metadata,
            before_selection,
            explicit_after_selection,
            undo_edits,
            redo_edits,
        })
    }

    fn cancel_composition_for_transaction(
        &mut self,
        source: TransactionSource,
    ) -> EngineResult<()> {
        if source != TransactionSource::Composition {
            self.cancel_composition_before_text_edit()?;
        }

        Ok(())
    }

    fn verify_transaction_base_version(&self, base_version: BufferVersion) -> EngineResult<()> {
        if base_version != self.version {
            return Err(crate::TransactionError::VersionMismatch {
                expected: self.version,
                actual: base_version,
            }
            .into());
        }

        Ok(())
    }

    fn commit_prepared_transaction(
        &mut self,
        prepared: &PreparedTransaction,
    ) -> EngineResult<(Delta, ChangeSet)> {
        self.apply_edit_list(prepared.base_version, prepared.edits.clone())
    }

    fn resolve_after_selection(
        &self,
        before_selection: &SelectionSet,
        explicit_after_selection: Option<&SelectionSet>,
        changeset: &ChangeSet,
    ) -> SelectionSet {
        explicit_after_selection
            .cloned()
            .unwrap_or_else(|| before_selection.map_through_changeset(changeset))
    }

    fn finish_transaction(
        &mut self,
        prepared: PreparedTransaction,
        after_selection: SelectionSet,
    ) -> EngineResult<()> {
        if prepared.metadata.record_history {
            let entry = HistoryEntry::new(
                prepared.undo_edits,
                prepared.redo_edits,
                prepared.before_selection,
                after_selection,
                prepared.metadata.description.clone(),
            );
            self.push_history(entry, &prepared.metadata)?;
            return Ok(());
        }

        // 任何新的文本变异都会让已有 redo 分支失效；Undo / Redo 自身走
        // apply_edit_list，不会触发这里。
        self.history.clear_redo();
        Ok(())
    }

    pub(in crate::buffer) fn apply_edit_list(
        &mut self,
        base_version: BufferVersion,
        tx_edits: EditList,
    ) -> EngineResult<(Delta, ChangeSet)> {
        self.ensure_writable()?;

        if base_version != self.version {
            return Err(crate::TransactionError::VersionMismatch {
                expected: self.version,
                actual: base_version,
            }
            .into());
        }

        // 1. 预检查：所有 edit 必须在当前旧文本字符坐标系中合法。
        self.validate_edit_list(&tx_edits)?;

        let edits = tx_edits.as_slice().to_vec();
        let old_version = self.version;

        // 2. 在 clone 上应用，确保未来 storage.replace 失败时不污染当前 Buffer。
        let mut new_storage = self.storage.clone();

        let mut reverse_edits = edits;
        reverse_edits.reverse();

        for edit in reverse_edits {
            new_storage.replace(edit.range, &edit.replacement)?;
        }

        // 3. 全部成功后再一次性提交 storage / version。
        self.storage = new_storage;
        self.bump_version()?;

        let new_version = self.version;

        let changeset = ChangeSet::from_edit_list(&tx_edits);

        let delta = Delta {
            old_version,
            new_version,
            edits: tx_edits,
        };

        Ok((delta, changeset))
    }
}
