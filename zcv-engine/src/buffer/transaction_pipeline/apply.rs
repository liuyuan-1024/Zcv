//! 事务应用管线：从 Transaction 校验、准备、提交到 history 收尾的一站式执行路径。
//!
//! 本文件守住失败原子性和版本推进边界；EditList 归一化、存储实现和 public 便利编辑入口不在这里定义。

use crate::{
    BufferVersion, EngineError, EngineResult, LargeTransactionPolicy, TransactionOutcome,
    errors::{EditError, StorageError, TransactionError},
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
        .map(|edit| edit.replacement().len())
        .sum()
}

impl Buffer {
    /// 提交并应用事务。
    ///
    /// 成功将返回增量 Delta 和事务变更集合 ChangeSet，并记录 Undo 历史。
    pub fn apply_transaction(&mut self, tx: Transaction) -> EngineResult<(Delta, ChangeSet)> {
        Ok(self.apply_transaction_outcome(tx)?.into_delta_changeset())
    }

    /// 提交并应用事务，返回事务身份、历史归属和增量事实。
    pub fn apply_transaction_outcome(
        &mut self,
        tx: Transaction,
    ) -> EngineResult<TransactionOutcome> {
        let (_, outcome) = self.apply_transaction_inner(tx)?;
        Ok(outcome)
    }

    /// 提交并应用事务，返回完整的可回放事实 `TransactionRecord`。
    pub fn apply_transaction_recorded(
        &mut self,
        tx: Transaction,
    ) -> EngineResult<TransactionRecord> {
        let (record, _) = self.apply_transaction_inner(tx)?;
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

        self.apply_transaction_recorded(record.to_transaction()?)
    }

    fn apply_transaction_inner(
        &mut self,
        tx: Transaction,
    ) -> EngineResult<(TransactionRecord, TransactionOutcome)> {
        self.ensure_writable()?;
        let mut prepared = self.prepare_transaction(tx)?;
        self.apply_large_transaction_policy(&mut prepared)?;

        // `Arc<[T]>` 让以下所有 clone 都是 O(1) 引用计数递增，无堆分配。
        // 仍然显式列出便于读者理解所有权流动；编译器不会自动 elide 这些 Arc::clone。
        let (transaction_id, delta, changeset) = self.apply_edit_list(
            prepared.base_version,
            prepared.edits.clone(),
            prepared.metadata.source(),
        )?;

        // 构造 TransactionRecord：所有字段都是 Arc-backed 或 Copy，clone 是 O(1)。
        let record = TransactionRecord::new(
            transaction_id,
            delta.old_version(),
            delta.new_version(),
            prepared.edits.clone(),
            prepared.undo_edits.clone(),
            prepared.metadata.clone(),
        );

        let history_transaction_id = self.finish_transaction(prepared, transaction_id)?;
        let outcome =
            TransactionOutcome::new(transaction_id, history_transaction_id, delta, changeset);

        Ok((record, outcome))
    }

    fn prepare_transaction(&mut self, tx: Transaction) -> EngineResult<PreparedTransaction> {
        let (base_version, edits, metadata) = tx.into_parts();

        self.verify_transaction_base_version(base_version)?;
        self.validate_edit_list(&edits)?;

        let undo_edits = self.build_inverse_edit_list(&edits)?;
        let redo_edits = edits.clone();

        Ok(PreparedTransaction {
            base_version,
            edits,
            metadata,
            undo_edits,
            redo_edits,
        })
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

    fn finish_transaction(
        &mut self,
        prepared: PreparedTransaction,
        transaction_id: crate::TransactionId,
    ) -> EngineResult<Option<crate::TransactionId>> {
        if prepared.metadata.record_history() {
            // Arc::clone：description 字符串只在历史节点持有一份共享
            let description = prepared.metadata.description_arc().cloned();
            let entry = HistoryEntry::new(
                transaction_id,
                prepared.undo_edits,
                prepared.redo_edits,
                description,
            );
            return self.push_history(entry, &prepared.metadata);
        }

        // record_history=false 提交后，当前节点下的 redo 分支已经基于过期文本，
        // 整体丢弃以避免后续 redo 走到不一致状态；undo 路径保持不变。
        self.drop_unrecorded_redo_branches();
        Ok(None)
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

    /// 把已校验的 `EditList` 落地到 Buffer。
    ///
    /// **半提交修复**：在 Buffer 本体变异**之前**完成所有可失败步骤（version 检查、
    /// validate、事务 id 溢出检查、`version.next()` 算溢出、prepared replace 容量预约、
    /// 后端边界预检与坐标换算、事件队列容量预约、Delta/ChangeSet 构造）。
    /// 文本内容先在 cloned storage 上完整构造；真正提交时只做 move assignment、
    /// 标量状态推进和已预留 Vec slot 写入，事务管线不再允许
    /// "Buffer 文本已经改了一半才返回 Result" 的状态机形态。
    ///
    /// `RopeyStorage::clone()` 是低成本共享底层结构；这里把它作为两阶段提交的
    /// prepared storage，而不是失败后的回滚补丁。
    pub(in crate::buffer) fn apply_edit_list(
        &mut self,
        base_version: BufferVersion,
        tx_edits: EditList,
        source: TransactionSource,
    ) -> EngineResult<(crate::TransactionId, Delta, ChangeSet)> {
        // ===== Fallible 段：在 Buffer 本体变异前完成全部可失败检查 =====
        self.ensure_writable()?;

        if base_version != self.version {
            return Err(TransactionError::VersionMismatch {
                expected: self.version,
                actual: base_version,
            }
            .into());
        }

        self.validate_edit_list(&tx_edits)?;
        let (transaction_id, next_transaction_id) = self.prepare_transaction_id()?;
        let old_version = self.version;
        let new_version = old_version.next().ok_or(EngineError::VersionOverflow)?;
        let prepared_replaces = self.prepare_storage_replaces(&tx_edits)?;
        self.reserve_delta_event_slot()?;

        let mut next_storage = self.storage.clone();
        for (edit, prepared_replace) in tx_edits
            .as_slice()
            .iter()
            .rev()
            .zip(prepared_replaces.into_iter().rev())
        {
            next_storage.replace_prepared(prepared_replace, edit.replacement());
        }

        let changeset = ChangeSet::from_edit_list(&tx_edits);
        let position_map = changeset.position_map();
        let delta = Delta::new(old_version, new_version, tx_edits);
        let pending_event = DeltaEvent::new(
            transaction_id,
            source,
            delta.clone(),
            changeset.clone(),
            position_map,
        );
        let last_event = pending_event.clone();

        // ===== Infallible 段：从这里起 Buffer 本体变异不允许失败 =====
        // 文本已经在 clone storage 上完整构造；真正提交只做 move assignment 与已预留队列写入。
        self.storage = next_storage;
        self.version = new_version;
        self.commit_delta_event(next_transaction_id, last_event, pending_event);

        Ok((transaction_id, delta, changeset))
    }

    fn prepare_storage_replaces(
        &self,
        tx_edits: &EditList,
    ) -> EngineResult<Vec<<crate::storage::RopeyStorage as TextStorage>::PreparedReplace>> {
        let mut prepared_replaces = Vec::new();
        prepared_replaces
            .try_reserve(tx_edits.len())
            .map_err(|_| StorageError::OutOfMemory)?;

        for edit in tx_edits.as_slice() {
            prepared_replaces.push(
                self.storage
                    .prepare_replace(edit.range(), edit.replacement())?,
            );
        }

        Ok(prepared_replaces)
    }
}
