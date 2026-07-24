//! Selection 编辑结果：返回文本事务事实和调用方应采用的新 SelectionSet。

use crate::{SelectionSet, TransactionId, TransactionOutcome};

/// 一次基于 SelectionSet 的编辑结果。
///
/// `transaction` 为 `None` 表示文本未变化；`after_selections` 仍然有效，
/// 调用方应更新自己的视图选区。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditOutcome {
    transaction: Option<TransactionOutcome>,
    after_selections: SelectionSet,
}

impl EditOutcome {
    pub(crate) fn unchanged(after_selections: SelectionSet) -> Self {
        Self {
            transaction: None,
            after_selections,
        }
    }

    pub(crate) fn edited(transaction: TransactionOutcome, after_selections: SelectionSet) -> Self {
        Self {
            transaction: Some(transaction),
            after_selections,
        }
    }

    pub fn transaction(&self) -> Option<&TransactionOutcome> {
        self.transaction.as_ref()
    }

    pub fn transaction_id(&self) -> Option<TransactionId> {
        self.transaction
            .as_ref()
            .map(TransactionOutcome::transaction_id)
    }

    pub fn history_transaction_id(&self) -> Option<TransactionId> {
        self.transaction
            .as_ref()
            .and_then(TransactionOutcome::history_transaction_id)
    }

    pub fn after_selections(&self) -> &SelectionSet {
        &self.after_selections
    }

    pub fn into_after_selections(self) -> SelectionSet {
        self.after_selections
    }
}
