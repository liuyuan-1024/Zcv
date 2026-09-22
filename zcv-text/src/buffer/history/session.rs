//! 事务会话：把会话内的多次编辑合并为单个历史节点。
//!
//! 会话只记录起始版本与事务身份；会话内每次编辑的可重放事实已由 `EditLog` 按版本保存，
//! `end_transaction` 时用版本区间构造单个历史节点。

use super::HistoryEntry;
use crate::{Buffer, BufferVersion, TextResult, TransactionId, transaction::TransactionMetadata};

/// 进行中的编辑会话。
#[derive(Debug, Clone)]
pub(in crate::buffer) struct TransactionSession {
    /// 会话历史节点的事务身份（`start_transaction` 时从 id 序列分配）。
    pub(in crate::buffer) transaction_id: TransactionId,
    /// 会话开始前的 BufferVersion。
    start_version: BufferVersion,
    /// 会话内最后一次编辑的元数据（end 时的合并策略与描述来源）。
    metadata: TransactionMetadata,
    /// 会话内是否仍有可记录历史的编辑（超大事务 SkipHistory 会关闭整个会话）。
    record_history: bool,
}

impl TransactionSession {
    fn new(transaction_id: TransactionId, start_version: BufferVersion) -> Self {
        Self {
            transaction_id,
            start_version,
            metadata: TransactionMetadata::default(),
            record_history: true,
        }
    }

    pub(in crate::buffer) fn record_edit(&mut self, metadata: &TransactionMetadata) {
        self.metadata = metadata.clone();
    }

    pub(in crate::buffer) fn discard_history(&mut self) {
        self.record_history = false;
    }

    pub(in crate::buffer) fn history_transaction_id(&self) -> Option<TransactionId> {
        self.record_history.then_some(self.transaction_id)
    }
}

impl Buffer {
    /// 开启编辑会话：会话内的多次编辑合并为单个历史节点，`end_transaction` 时写入历史。
    ///
    /// 幂等：会话已开启时返回 `None`。返回的会话事务身份供宿主在会话边界记录视图状态。
    pub fn start_transaction(&mut self) -> TextResult<Option<TransactionId>> {
        if self.session.is_some() {
            return Ok(None);
        }
        let (transaction_id, next_transaction_id) = self.prepare_transaction_id()?;
        self.next_transaction_id = next_transaction_id;
        self.session = Some(TransactionSession::new(transaction_id, self.version));
        Ok(Some(transaction_id))
    }

    /// 提交编辑会话：把会话内累积的版本区间合并为单个历史节点，返回节点的事务身份。
    ///
    /// 空会话、会话内编辑全部被 SkipHistory 放弃时返回 `None`，不产生历史节点。
    pub fn end_transaction(&mut self) -> TextResult<Option<TransactionId>> {
        let Some(session) = self.session.take() else {
            return Ok(None);
        };
        let recorded = if !session.record_history {
            // 会话内出现放弃历史的编辑：文本已前进，redo 分支失效。
            self.drop_unrecorded_redo_branches();
            false
        } else if self.version == session.start_version {
            // 空会话：无文本变化，不产生历史节点，也不影响 redo 分支。
            false
        } else {
            let entry = HistoryEntry::new(
                session.transaction_id,
                session.start_version,
                self.version,
                session.metadata.description_arc().cloned(),
            );
            self.push_history(entry, &session.metadata)?;
            true
        };
        self.truncate_edit_history_to_budget();
        if recorded {
            Ok(self.history.current_transaction_id())
        } else {
            Ok(None)
        }
    }
}
