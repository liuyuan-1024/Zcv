//! 事务结果：把一次已提交文本变更的身份、历史归属和增量事实返回给宿主。

use crate::{ChangeSet, Delta, DeltaEvent, TransactionId};

/// 一次实际文本提交的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionOutcome {
    transaction_id: TransactionId,
    history_transaction_id: Option<TransactionId>,
    delta: Delta,
    changeset: ChangeSet,
    event: DeltaEvent,
}

impl TransactionOutcome {
    pub(crate) fn new(
        transaction_id: TransactionId,
        history_transaction_id: Option<TransactionId>,
        delta: Delta,
        changeset: ChangeSet,
        event: DeltaEvent,
    ) -> Self {
        Self {
            transaction_id,
            history_transaction_id,
            delta,
            changeset,
            event,
        }
    }

    /// 本次实际文本提交的身份。
    pub fn transaction_id(&self) -> TransactionId {
        self.transaction_id
    }

    /// 对应 Undo / Redo 历史节点的规范事务身份。
    ///
    /// 未记录历史时返回 `None`；与前一事务合并时返回前一历史事务的身份。
    pub fn history_transaction_id(&self) -> Option<TransactionId> {
        self.history_transaction_id
    }

    pub fn delta(&self) -> &Delta {
        &self.delta
    }

    pub fn changeset(&self) -> &ChangeSet {
        &self.changeset
    }

    /// 本次提交对应的单步事件事实；跨多次提交的观察者应使用 `Buffer::subscribe`。
    pub fn event(&self) -> &DeltaEvent {
        &self.event
    }
}
