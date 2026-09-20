//! 事务应用管线：从 Transaction 校验、准备、提交到 history 收尾的一站式执行路径。
//!
//! 本文件守住失败原子性和版本推进边界；EditList 归一化、存储实现和 public edit 入口不在这里定义。

use super::prepared::PreparedTransaction;
use crate::buffer::{Buffer, history::HistoryEntry};
use crate::{
    config::LargeTransactionPolicy,
    errors::{EditError, StorageError, TransactionError},
    errors::{TextError, TextResult},
    storage::{RopeyPreparedReplace, RopeyStorage},
    text_changes::TextPatch,
    transaction::TransactionOutcome,
    transaction::{
        ChangeSet, Delta, DeltaEvent, EditList, Transaction, TransactionMetadata, TransactionSource,
    },
    types::{BufferGeneration, BufferVersion},
};

impl Buffer {
    /// 提交并应用事务。
    ///
    /// 成功返回事务身份、历史归属和增量事实，并按事务元数据记录 Undo 历史。
    pub(crate) fn apply_transaction(&mut self, tx: Transaction) -> TextResult<TransactionOutcome> {
        self.ensure_writable()?;
        let (mut prepared, next_transaction_id, event) = self.prepare_transaction(tx)?;
        self.apply_large_transaction_policy(&mut prepared)?;

        // 进入历史的事务才保存逆编辑；SkipHistory 大事务不白存被删文本。
        let undo_edits = self
            .records_history(&prepared.metadata)
            .then(|| prepared.undo_edits.clone());
        self.commit_prepared_edit_list(&prepared.edits, undo_edits, next_transaction_id, &event)?;

        let history_transaction_id = self.finish_transaction(
            prepared,
            event.transaction_id(),
            event.old_version(),
            event.new_version(),
        )?;
        Ok(TransactionOutcome::new(history_transaction_id, event))
    }

    fn prepare_transaction(
        &self,
        tx: Transaction,
    ) -> TextResult<(PreparedTransaction, crate::TransactionId, DeltaEvent)> {
        let (base_version, edits, metadata) = tx.into_parts();
        let (next_transaction_id, event) =
            self.prepare_delta_event(base_version, edits.clone(), metadata.source(), false)?;
        let undo_edits = self.build_inverse_edit_list(&edits)?;

        Ok((
            PreparedTransaction {
                edits,
                metadata,
                undo_edits,
            },
            next_transaction_id,
            event,
        ))
    }

    /// 本次编辑是否进入历史：事务自身要求记录，且当前会话没有放弃历史。
    fn records_history(&self, metadata: &TransactionMetadata) -> bool {
        if !metadata.record_history() {
            return false;
        }
        match &self.session {
            Some(session) => session.history_transaction_id().is_some(),
            None => true,
        }
    }

    fn finish_transaction(
        &mut self,
        prepared: PreparedTransaction,
        transaction_id: crate::TransactionId,
        old_version: BufferVersion,
        new_version: BufferVersion,
    ) -> TextResult<Option<crate::TransactionId>> {
        let records_history = self.records_history(&prepared.metadata);
        if let Some(session) = &mut self.session {
            // 会话内：历史写入推迟到 `end_transaction`，这里只延续/放弃会话记录。
            // 不在会话内裁剪日志，否则会丢掉本会话更早版本、导致 `end_transaction` 后无法回放。
            if records_history {
                session.record_edit(&prepared.metadata);
            } else {
                // 超大事务放弃历史（SkipHistory）：整个会话的历史作废，否则 undo 回放会漏掉会话内的这些文本变化。
                session.discard_history();
            }
            return Ok(session.history_transaction_id());
        }

        if prepared.metadata.record_history() {
            // Arc::clone：description 字符串只在历史节点持有一份共享
            let description = prepared.metadata.description_arc().cloned();
            let entry = HistoryEntry::new(transaction_id, old_version, new_version, description);
            self.push_history(entry, &prepared.metadata)?;
            self.truncate_edit_history_to_budget();
            return Ok(self.history.current_transaction_id());
        }

        // record_history=false 提交后，当前节点下的 redo 分支已经基于过期文本，
        // 整体丢弃以避免后续 redo 走到不一致状态；undo 路径保持不变。
        self.drop_unrecorded_redo_branches();
        self.truncate_edit_history_to_budget();
        Ok(None)
    }

    /// 在 prepare 之后、commit 之前，按 `LargeFilePolicy` 处理超大事务。
    ///
    /// `Reject`：原子拒绝事务，文本 / 版本 / 历史完全不变。
    /// `SkipHistory`：把 metadata 的 `record_history` 关掉，复用既有
    /// `records_history` 中 `record_history=false` 路径，文本前进但不入历史
    /// 且丢弃当前节点子树。
    fn apply_large_transaction_policy(&self, prepared: &mut PreparedTransaction) -> TextResult<()> {
        let threshold = self.config.large_file.large_transaction_threshold_bytes;
        if threshold == 0 {
            return Ok(());
        }

        let entry_bytes =
            prepared.edits.replacement_bytes() + prepared.undo_edits.replacement_bytes();
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
    ) -> TextResult<DeltaEvent> {
        // ===== Fallible 段：在 Buffer 本体变异前完成全部可失败检查 =====
        self.ensure_writable()?;

        let (next_transaction_id, event) =
            self.prepare_delta_event(base_version, tx_edits.clone(), source, false)?;
        self.commit_prepared_edit_list(&tx_edits, None, next_transaction_id, &event)?;
        Ok(event)
    }

    /// 将已验证并已绑定事务身份的编辑落到克隆存储，再原子替换 Buffer 状态。
    fn commit_prepared_edit_list(
        &mut self,
        forward: &EditList,
        undo: Option<EditList>,
        next_transaction_id: crate::TransactionId,
        event: &DeltaEvent,
    ) -> TextResult<()> {
        let prepared_replaces = self.prepare_storage_replaces(forward)?;
        let mut next_storage = self.storage.clone();
        for (edit, prepared_replace) in forward
            .as_slice()
            .iter()
            .rev()
            .zip(prepared_replaces.into_iter().rev())
        {
            next_storage.replace_prepared(prepared_replace, edit.replacement());
        }

        // ===== Commit 段：从这里起 Buffer 本体变异不允许失败 =====
        // 文本已经在 clone storage 上完整构造；真正提交只做 move assignment 与订阅发布。
        self.commit_prepared_text_change(
            next_storage,
            forward.clone(),
            undo,
            next_transaction_id,
            event,
        );
        Ok(())
    }

    /// 为一次已确定的文本变化构造唯一的版本、事务与坐标映射事实。
    ///
    /// 普通编辑和外部基线重载共用该边界；两者只在历史策略和 reset 语义上不同。
    pub(in crate::buffer) fn prepare_delta_event(
        &self,
        base_version: BufferVersion,
        tx_edits: EditList,
        source: TransactionSource,
        reset: bool,
    ) -> TextResult<(crate::TransactionId, DeltaEvent)> {
        if base_version != self.version {
            return Err(TransactionError::VersionMismatch {
                expected: self.version,
                actual: base_version,
            }
            .into());
        }

        self.validate_edit_list(&tx_edits)?;
        let (transaction_id, next_transaction_id) = self.prepare_transaction_id()?;
        let new_version = base_version.next().ok_or(TextError::VersionOverflow)?;
        let changeset = ChangeSet::from_edit_list(&tx_edits);
        let position_map = changeset.position_map();
        let delta = Delta::new(base_version, new_version, tx_edits);
        let event = DeltaEvent::new(
            transaction_id,
            source,
            delta,
            changeset,
            position_map,
            reset,
        );
        Ok((next_transaction_id, event))
    }

    pub(in crate::buffer) fn commit_prepared_text_change(
        &mut self,
        next_storage: RopeyStorage,
        forward: EditList,
        undo: Option<EditList>,
        next_transaction_id: crate::TransactionId,
        event: &DeltaEvent,
    ) {
        self.storage = next_storage;
        self.version = event.new_version();
        // reset / 基线替换开启新代际：普通解析拒绝把旧代际锚点当作普通编辑继续映射；
        // 跨代际映射仍保留在坐标索引中，供调用方显式重锚（例如外部 reload 后的光标恢复）。
        if event.requires_reset() {
            self.generation = BufferGeneration::new(event.new_version());
        }
        // 不衰减坐标索引与带文本 EditLog 一起追加；预算裁剪只作用于后者。
        self.coordinate_index = self.coordinate_index.appended(
            event.old_version(),
            event.new_version(),
            TextPatch::from_delta(event.delta()),
        );
        // 编辑日志是版本化编辑的唯一事实：组合文档增量同步与历史回放都据此重建坐标。
        self.edit_log = self.edit_log.appended(
            event.old_version(),
            event.new_version(),
            forward,
            undo,
            event.requires_reset(),
        );
        self.commit_delta_event(next_transaction_id, event);
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

#[cfg(test)]
#[path = "test/apply_tests.rs"]
mod tests;
