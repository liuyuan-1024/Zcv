//! 事务会话：把会话内的多次编辑合并为单个历史节点。
//!
//! 会话在编辑前开启并分配历史节点的事务身份，会话内的编辑只累积 undo/redo 批次，结束时一次性写入历史图。

use super::HistoryEntry;
use crate::{
    Buffer, EngineResult, TransactionId,
    transaction::{EditList, TransactionMetadata},
};

/// 进行中的编辑会话。
#[derive(Debug)]
pub(in crate::buffer) struct TransactionSession {
    /// 会话历史节点的事务身份（`start_transaction` 时从 id 序列分配）。
    pub(in crate::buffer) transaction_id: TransactionId,
    /// 会话内已累积的 undo / redo 批次（按编辑顺序）。
    undo_batches: Vec<EditList>,
    redo_batches: Vec<EditList>,
    /// 会话内最后一次编辑的元数据（end 时的合并策略与描述来源）。
    metadata: TransactionMetadata,
    /// 会话内是否仍有可记录历史的编辑（超大事务 SkipHistory 会关闭整个会话）。
    record_history: bool,
}

impl TransactionSession {
    fn new(transaction_id: TransactionId) -> Self {
        Self {
            transaction_id,
            undo_batches: Vec::new(),
            redo_batches: Vec::new(),
            metadata: TransactionMetadata::default(),
            record_history: true,
        }
    }

    pub(in crate::buffer) fn append(
        &mut self,
        undo_edits: EditList,
        redo_edits: EditList,
        metadata: &TransactionMetadata,
    ) {
        self.undo_batches.push(undo_edits);
        self.redo_batches.push(redo_edits);
        self.metadata = metadata.clone();
    }

    pub(in crate::buffer) fn discard_history(&mut self) {
        self.undo_batches.clear();
        self.redo_batches.clear();
        self.record_history = false;
    }
}

impl Buffer {
    /// 开启编辑会话：会话内的多次编辑合并为单个历史节点，`end_transaction` 时写入历史。
    ///
    /// 幂等：会话已开启时返回 `None`。返回的会话事务身份供宿主在会话边界记录视图状态。
    pub fn start_transaction(&mut self) -> EngineResult<Option<TransactionId>> {
        if self.session.is_some() {
            return Ok(None);
        }
        let (transaction_id, next_transaction_id) = self.prepare_transaction_id()?;
        self.next_transaction_id = next_transaction_id;
        self.session = Some(TransactionSession::new(transaction_id));
        Ok(Some(transaction_id))
    }

    /// 提交编辑会话：把会话内累积的批次合并为单个历史节点，返回节点的事务身份。
    ///
    /// 空会话、会话内编辑全部被 SkipHistory 放弃时返回 `None`，不产生历史节点。
    pub fn end_transaction(&mut self) -> EngineResult<Option<TransactionId>> {
        let Some(mut session) = self.session.take() else {
            return Ok(None);
        };
        if !session.record_history {
            // 会话内出现放弃历史的编辑：文本已前进，redo 分支失效。
            self.drop_unrecorded_redo_branches();
            return Ok(None);
        }
        if session.undo_batches.is_empty() {
            // 空会话：无文本变化，不产生历史节点，也不影响 redo 分支。
            return Ok(None);
        }

        // undo 回放必须逆序（后编辑先撤销），与 `HistoryEntry::merge` 的批次顺序一致。
        session.undo_batches.reverse();

        let entry = HistoryEntry::from_batches(
            session.transaction_id,
            session.undo_batches,
            session.redo_batches,
            session.metadata.description_arc().cloned(),
        );
        self.push_history(entry, &session.metadata)
    }
}
