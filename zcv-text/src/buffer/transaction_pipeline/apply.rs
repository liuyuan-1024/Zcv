//! 事务应用管线：从 Transaction 校验、准备、提交到 history 收尾的一站式执行路径。
//!
//! 本文件守住失败原子性和版本推进边界；EditList 归一化、存储实现和 public edit 入口不在这里定义。

use super::prepared::PreparedTransaction;
use crate::buffer::{Buffer, history::HistoryEntry};
use crate::{
    config::LargeTransactionPolicy,
    errors::{EditError, StorageError, TransactionError},
    errors::{TextError, TextResult},
    storage::{RopeyPreparedReplace, TextStorage},
    transaction::TransactionOutcome,
    transaction::{ChangeSet, Delta, DeltaEvent, EditList, Transaction, TransactionSource},
    types::BufferVersion,
};

/// 单个 `EditList` 内所有 `Edit::replacement` 的 UTF-8 字节和。
///
/// 避免事务 prepare 阶段的预算检查与最终 `HistoryEntry` 的字节统计漂移。
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
    /// 成功返回事务身份、历史归属和增量事实，并按事务元数据记录 Undo 历史。
    pub(crate) fn apply_transaction(&mut self, tx: Transaction) -> TextResult<TransactionOutcome> {
        self.ensure_writable()?;
        let mut prepared = self.prepare_transaction(tx)?;
        self.apply_large_transaction_policy(&mut prepared)?;

        let (transaction_id, _delta, _changeset, event) = self.apply_edit_list(
            prepared.base_version,
            prepared.edits.clone(),
            prepared.metadata.source(),
        )?;

        let history_transaction_id = self.finish_transaction(prepared, transaction_id)?;
        Ok(TransactionOutcome::new(history_transaction_id, event))
    }

    fn prepare_transaction(&mut self, tx: Transaction) -> TextResult<PreparedTransaction> {
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

    fn verify_transaction_base_version(&self, base_version: BufferVersion) -> TextResult<()> {
        if base_version != self.version {
            return Err(TransactionError::VersionMismatch {
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
    ) -> TextResult<Option<crate::TransactionId>> {
        if let Some(session) = &mut self.session {
            // 会话内：只累积 undo/redo 批次，历史写入推迟到 `end_transaction`。
            if prepared.metadata.record_history() {
                session.append(prepared.undo_edits, prepared.redo_edits, &prepared.metadata);
            } else {
                // 超大事务放弃历史（SkipHistory）：整个会话的历史作废，否则 undo 回放会漏掉会话内的这些文本变化。
                session.discard_history();
            }
            return Ok(Some(session.transaction_id));
        }

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
    fn apply_large_transaction_policy(&self, prepared: &mut PreparedTransaction) -> TextResult<()> {
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
    /// 后端边界预检与坐标换算、Delta/ChangeSet/Patch 构造）。
    /// 文本内容先在 cloned storage 上完整构造；真正提交时只做 move assignment、
    /// 标量状态推进和订阅发布，事务管线不再允许
    /// "Buffer 文本已经改了一半才返回 Result" 的状态机形态。
    ///
    /// `RopeyStorage::clone()` 是低成本共享底层结构；这里把它作为两阶段提交的
    /// prepared storage，而不是失败后的回滚补丁。
    pub(in crate::buffer) fn apply_edit_list(
        &mut self,
        base_version: BufferVersion,
        tx_edits: EditList,
        source: TransactionSource,
    ) -> TextResult<(crate::TransactionId, Delta, ChangeSet, DeltaEvent)> {
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
        let new_version = old_version.next().ok_or(TextError::VersionOverflow)?;
        let prepared_replaces = self.prepare_storage_replaces(&tx_edits)?;
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
        let event = DeltaEvent::new(
            transaction_id,
            source,
            delta.clone(),
            changeset.clone(),
            position_map,
        );

        // ===== Commit 段：从这里起 Buffer 本体变异不允许失败 =====
        // 文本已经在 clone storage 上完整构造；真正提交只做 move assignment 与订阅发布。
        self.storage = next_storage;
        self.version = new_version;
        self.commit_delta_event(next_transaction_id, &event);

        Ok((transaction_id, delta, changeset, event))
    }

    fn prepare_storage_replaces(
        &self,
        tx_edits: &EditList,
    ) -> TextResult<Vec<RopeyPreparedReplace>> {
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
