//! 事务应用管线：把 Transaction 规划为完整派生状态，再原子安装。
//!
//! 规划阶段只读取 Buffer，并在克隆的存储、日志、坐标索引、历史上完成全部可失败步骤；
//! 安装阶段只做 move 换入与订阅发布。EditList 归一化、存储实现和 public edit 入口不在这里定义。

use super::prepared::{DerivedBufferState, PreparedTransaction};
use crate::buffer::Buffer;
use crate::buffer::history::{
    HistoryEntry, HistoryState, TransactionSession, push_history_into, truncate_edit_history,
};
use crate::{
    config::LargeTransactionPolicy,
    errors::{EditError, TextError, TextResult, TransactionError},
    text_changes::TextPatch,
    tracking::EditLog,
    transaction::TransactionOutcome,
    transaction::{
        ChangeSet, Delta, DeltaEvent, EditList, Transaction, TransactionMetadata, TransactionSource,
    },
    types::BufferVersion,
};

/// 历史和会话收尾所需的事务事实及其派生状态。
///
/// 这些引用共同表示一次事务规划的收尾边界，避免让收尾函数逐项接收互相关联的状态。
struct TransactionFinalization<'a> {
    metadata: &'a TransactionMetadata,
    event: &'a DeltaEvent,
    edit_log: &'a mut EditLog,
    history: &'a mut HistoryState,
    session: &'a mut Option<TransactionSession>,
}

impl Buffer {
    /// 提交并应用事务。
    pub(crate) fn apply_transaction(&mut self, tx: Transaction) -> TextResult<TransactionOutcome> {
        let derived = self.plan_transaction(tx)?;
        let (history_transaction_id, event) = self.install(derived);
        Ok(TransactionOutcome::new(history_transaction_id, event))
    }

    /// 计算事务的完整派生状态；只读取 Buffer，不产生任何变异。
    pub(in crate::buffer) fn plan_transaction(
        &self,
        tx: Transaction,
    ) -> TextResult<DerivedBufferState> {
        self.ensure_writable()?;
        let (mut prepared, next_transaction_id, event) = self.prepare_transaction(tx)?;
        self.apply_large_transaction_policy(&mut prepared)?;

        let undo_edits = self
            .records_history(&prepared.metadata)
            .then(|| prepared.undo_edits.clone());
        self.plan_edit_list_state(
            &prepared.edits,
            undo_edits,
            next_transaction_id,
            event,
            Some(&prepared.metadata),
            None,
        )
    }

    fn prepare_transaction(
        &self,
        tx: Transaction,
    ) -> TextResult<(PreparedTransaction, crate::TransactionId, DeltaEvent)> {
        let (base_version, edits, metadata) = tx.into_parts();
        let (next_transaction_id, event) =
            self.prepare_delta_event(base_version, edits.clone(), metadata.source())?;
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

    /// 在 prepare 之后、计划落地之前，按 LargeFilePolicy 处理超大事务。
    ///
    /// Reject：原子拒绝事务，文本 / 版本 / 历史完全不变。
    /// SkipHistory：把 metadata 的 record_history 关掉，文本前进但不入历史且丢弃当前节点子树。
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

    /// 在克隆状态上构造派生状态：存储、编辑日志、坐标索引、历史与会话收尾。
    ///
    /// `metadata` 为 None 表示回放路径（undo/redo），不改变历史图。
    fn plan_edit_list_state(
        &self,
        forward: &EditList,
        undo: Option<EditList>,
        next_transaction_id: crate::TransactionId,
        event: DeltaEvent,
        metadata: Option<&TransactionMetadata>,
        revert: Option<(BufferVersion, BufferVersion)>,
    ) -> TextResult<DerivedBufferState> {
        let mut next_storage = self.storage.clone();
        next_storage.apply_edit_list(forward)?;
        let next_insertions = match revert {
            Some((start, end)) => self.insertions.undone(start, end, event.new_version()),
            None => self.insertions.with_edits(forward, event.new_version()),
        };

        let mut next_edit_log = self.edit_log.appended(
            event.old_version(),
            event.new_version(),
            forward.clone(),
            undo,
        );
        let next_coordinate_index = self.coordinate_index.appended(
            event.old_version(),
            event.new_version(),
            TextPatch::from_delta(event.delta()),
        );

        let mut next_history = self.history.clone();
        let mut next_session = self.session.clone();
        let history_transaction_id = match metadata {
            Some(metadata) => self.plan_finish_transaction(TransactionFinalization {
                metadata,
                event: &event,
                edit_log: &mut next_edit_log,
                history: &mut next_history,
                session: &mut next_session,
            })?,
            None => None,
        };

        Ok(DerivedBufferState {
            storage: next_storage,
            version: event.new_version(),
            edit_log: next_edit_log,
            coordinate_index: next_coordinate_index,
            insertions: next_insertions,
            history: next_history,
            session: next_session,
            next_transaction_id,
            event,
            history_transaction_id,
        })
    }

    /// 历史 / 会话收尾的计划版本：只作用于传入的克隆状态。
    fn plan_finish_transaction(
        &self,
        finalization: TransactionFinalization<'_>,
    ) -> TextResult<Option<crate::TransactionId>> {
        let TransactionFinalization {
            metadata,
            event,
            edit_log,
            history,
            session,
        } = finalization;
        let records_history = self.records_history(metadata);
        if let Some(active) = session {
            // 会话内：历史写入推迟到 end_transaction，这里只延续/放弃会话记录。
            // 不在会话内裁剪日志，否则会丢掉本会话更早版本、导致 end_transaction 后无法回放。
            if records_history {
                active.record_edit(metadata);
            } else {
                active.discard_history();
            }
            return Ok(active.history_transaction_id());
        }

        if metadata.record_history() {
            let description = metadata.description_arc().cloned();
            let entry = HistoryEntry::new(
                event.transaction_id(),
                event.old_version(),
                event.new_version(),
                description,
            );
            push_history_into(history, edit_log, entry, metadata)?;
            truncate_edit_history(edit_log, history, &self.config.large_file);
            return Ok(history.current_transaction_id());
        }

        // record_history=false 后，当前节点下的 redo 分支已基于过期文本，整体丢弃。
        history.drop_children_of_current();
        truncate_edit_history(edit_log, history, &self.config.large_file);
        Ok(None)
    }

    /// 安装派生状态：整体换入并发布订阅批次；返回历史事务身份与事件。
    pub(in crate::buffer) fn install(
        &mut self,
        derived: DerivedBufferState,
    ) -> (Option<crate::TransactionId>, DeltaEvent) {
        let DerivedBufferState {
            storage,
            version,
            edit_log,
            coordinate_index,
            insertions,
            history,
            session,
            next_transaction_id,
            event,
            history_transaction_id,
        } = derived;

        self.storage = storage;
        self.version = version;
        self.edit_log = edit_log;
        self.coordinate_index = coordinate_index;
        self.insertions = insertions;
        self.history = history;
        self.session = session;
        self.commit_delta_event(next_transaction_id, &event);
        (history_transaction_id, event)
    }

    /// 回放（undo/redo）路径：把已校验的 EditList 规划并安装，不改变历史图。
    pub(in crate::buffer) fn apply_edit_list(
        &mut self,
        base_version: BufferVersion,
        tx_edits: EditList,
        source: TransactionSource,
        revert: Option<(BufferVersion, BufferVersion)>,
    ) -> TextResult<DeltaEvent> {
        let derived = self.plan_edit_list(base_version, tx_edits, source, revert)?;
        let (_, event) = self.install(derived);
        Ok(event)
    }

    fn plan_edit_list(
        &self,
        base_version: BufferVersion,
        tx_edits: EditList,
        source: TransactionSource,
        revert: Option<(BufferVersion, BufferVersion)>,
    ) -> TextResult<DerivedBufferState> {
        self.ensure_writable()?;
        let (next_transaction_id, event) =
            self.prepare_delta_event(base_version, tx_edits.clone(), source)?;
        // 回放与普通提交一样记录逆编辑：它的逆就是反向回放所需的编辑。
        let undo_edits = self.build_inverse_edit_list(&tx_edits)?;
        self.plan_edit_list_state(
            &tx_edits,
            Some(undo_edits),
            next_transaction_id,
            event,
            None,
            revert,
        )
    }

    /// 按给定元数据规划一次编辑列表：应用大事务策略并记录历史，但不安装。
    ///
    /// `snapshot_with_edits` 与外部文本更新共用此入口；安装由 `fast_forward` 或 `install` 完成。
    pub(in crate::buffer) fn plan_edit_list_with_metadata(
        &self,
        base_version: BufferVersion,
        edits: EditList,
        metadata: TransactionMetadata,
    ) -> TextResult<DerivedBufferState> {
        self.ensure_writable()?;
        let undo_edits = self.build_inverse_edit_list(&edits)?;
        let mut prepared = PreparedTransaction {
            edits,
            metadata,
            undo_edits,
        };
        self.apply_large_transaction_policy(&mut prepared)?;

        let (next_transaction_id, event) = self.prepare_delta_event(
            base_version,
            prepared.edits.clone(),
            prepared.metadata.source(),
        )?;
        let undo_edits = self
            .records_history(&prepared.metadata)
            .then(|| prepared.undo_edits.clone());
        self.plan_edit_list_state(
            &prepared.edits,
            undo_edits,
            next_transaction_id,
            event,
            Some(&prepared.metadata),
            None,
        )
    }

    /// 为一次已确定的文本变化构造唯一的版本、事务与坐标映射事实。
    pub(in crate::buffer) fn prepare_delta_event(
        &self,
        base_version: BufferVersion,
        tx_edits: EditList,
        source: TransactionSource,
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
        let event = DeltaEvent::new(transaction_id, source, delta, changeset, position_map);
        Ok((next_transaction_id, event))
    }
}

#[cfg(test)]
#[path = "test/apply_tests.rs"]
mod tests;
