//! 事务应用管线：从 Transaction 校验、准备、提交到 selection 映射和 history 收尾的一站式执行路径。
//!
//! 本文件守住失败原子性和版本推进边界；EditList 归一化、存储实现和 public 便利编辑入口不在这里定义。

use crate::{
    BufferVersion, EngineResult, LargeTransactionPolicy, PositionMap, SelectionSet,
    errors::{EditError, TransactionError},
    storage::TextStorage,
    transaction::{
        ChangeSet, Delta, DeltaEvent, EditList, Transaction, TransactionRecord, TransactionSource,
    },
};

use crate::buffer::{Buffer, history::HistoryEntry};

use super::prepared::PreparedTransaction;

/// 单个 `EditList` 内所有 `Edit::replacement` 的 UTF-8 字节和。
///
/// 度量口径与 `HistoryEntry::byte_size` 对齐，避免事务 prepare 阶段的预算检查
/// 与最终 `HistoryEntry` 的字节统计漂移。
pub(in crate::buffer) fn edit_list_replacement_bytes(edits: &EditList) -> usize {
    edits
        .as_slice()
        .iter()
        .map(|edit| edit.replacement.len())
        .sum()
}

impl Buffer {
    /// 提交并应用事务。
    ///
    /// 成功将返回增量 Delta 和事务变更集合 ChangeSet，并记录 Undo 历史。
    pub fn apply_transaction(&mut self, tx: Transaction) -> EngineResult<(Delta, ChangeSet)> {
        let (_, delta, changeset) = self.apply_transaction_inner(tx)?;
        Ok((delta, changeset))
    }

    /// 提交并应用事务，返回完整的可回放事实 `TransactionRecord`。
    pub fn apply_transaction_recorded(
        &mut self,
        tx: Transaction,
    ) -> EngineResult<TransactionRecord> {
        let (record, _, _) = self.apply_transaction_inner(tx)?;
        Ok(record)
    }

    /// 在当前 Buffer 上回放一条 `TransactionRecord`，必须 `record.old_version == self.version`。
    ///
    /// 回放走标准 `apply_transaction` 管线，不绕过任何边界校验；返回新生成的
    /// `TransactionRecord`（`transaction_id` 由当前 Buffer 重新分配）。
    pub fn replay_transaction_record(
        &mut self,
        record: &TransactionRecord,
    ) -> EngineResult<TransactionRecord> {
        if record.old_version() != self.version {
            return Err(TransactionError::VersionMismatch {
                expected: self.version,
                actual: record.old_version(),
            }
            .into());
        }

        self.apply_transaction_recorded(record.to_transaction())
    }

    fn apply_transaction_inner(
        &mut self,
        tx: Transaction,
    ) -> EngineResult<(TransactionRecord, Delta, ChangeSet)> {
        self.ensure_writable()?;
        let mut prepared = self.prepare_transaction(tx)?;
        self.apply_large_transaction_policy(&mut prepared)?;
        let edits_for_record = prepared.edits.clone();
        let undo_edits_for_record = prepared.undo_edits.clone();
        let before_selection_for_record = prepared.before_selection.clone();
        let metadata_for_record = prepared.metadata.clone();

        let (delta, changeset) = self.commit_prepared_transaction(&prepared)?;
        let position_map = changeset.position_map();
        let after_selection = self.resolve_after_selection(
            &prepared.before_selection,
            prepared.explicit_after_selection.as_ref(),
            &position_map,
        );
        let after_selection_for_record = after_selection.clone();
        self.selection = after_selection.clone();
        self.finish_transaction(prepared, after_selection)?;

        let event = self
            .last_delta_event()
            .expect("apply_transaction 提交成功后必然有 DeltaEvent");
        let record = TransactionRecord::new(
            event.transaction_id,
            delta.old_version,
            delta.new_version,
            edits_for_record,
            undo_edits_for_record,
            before_selection_for_record,
            after_selection_for_record,
            metadata_for_record,
        );
        Ok((record, delta, changeset))
    }

    fn prepare_transaction(&mut self, tx: Transaction) -> EngineResult<PreparedTransaction> {
        self.cancel_composition_for_transaction(tx.metadata().source())?;

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
        self.apply_edit_list(
            prepared.base_version,
            prepared.edits.clone(),
            prepared.metadata.source(),
        )
    }

    fn resolve_after_selection(
        &self,
        before_selection: &SelectionSet,
        explicit_after_selection: Option<&SelectionSet>,
        position_map: &PositionMap,
    ) -> SelectionSet {
        explicit_after_selection
            .cloned()
            .unwrap_or_else(|| before_selection.map_through_position_map(position_map))
    }

    fn finish_transaction(
        &mut self,
        prepared: PreparedTransaction,
        after_selection: SelectionSet,
    ) -> EngineResult<()> {
        if prepared.metadata.record_history() {
            let entry = HistoryEntry::new(
                prepared.undo_edits,
                prepared.redo_edits,
                prepared.before_selection,
                after_selection,
                prepared.metadata.description().map(str::to_string),
            );
            self.push_history(entry, &prepared.metadata)?;
            return Ok(());
        }

        // record_history=false 提交后，当前节点下的 redo 分支已经基于过期文本，
        // 整体丢弃以避免后续 redo 走到不一致状态；undo 路径保持不变。
        self.drop_unrecorded_redo_branches();
        Ok(())
    }

    /// 在 prepare 之后、commit 之前，按 `LargeFilePolicy` 处理超大事务。
    ///
    /// `Reject`：原子拒绝事务，文本 / 版本 / 历史完全不变。
    /// `SkipHistory`：把 metadata 的 `record_history` 关掉，复用既有
    /// `finish_transaction` 中 `record_history=false` 路径，文本前进但不入历史
    /// 且丢弃当前节点子树。
    fn apply_large_transaction_policy(
        &self,
        prepared: &mut PreparedTransaction,
    ) -> EngineResult<()> {
        let threshold = self.config.large_file.large_transaction_threshold_bytes;
        if threshold == 0 {
            return Ok(());
        }

        let entry_bytes = edit_list_replacement_bytes(&prepared.edits)
            + edit_list_replacement_bytes(&prepared.undo_edits);
        if entry_bytes <= threshold {
            return Ok(());
        }

        match self.config.large_file.large_transaction_policy {
            LargeTransactionPolicy::Reject => Err(EditError::PayloadTooLarge {
                size: entry_bytes,
                limit: threshold,
            }
            .into()),
            LargeTransactionPolicy::SkipHistory => {
                prepared.metadata = prepared.metadata.clone().without_history();
                Ok(())
            }
        }
    }

    pub(in crate::buffer) fn apply_edit_list(
        &mut self,
        base_version: BufferVersion,
        tx_edits: EditList,
        source: TransactionSource,
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
        let transaction_id = self.reserve_transaction_id()?;

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
        let position_map = changeset.position_map();

        let delta = Delta {
            old_version,
            new_version,
            edits: tx_edits,
        };

        self.push_delta_event(DeltaEvent {
            transaction_id,
            old_version,
            new_version,
            source,
            delta: delta.clone(),
            changeset: changeset.clone(),
            position_map,
        });

        Ok((delta, changeset))
    }
}
