//! TransactionRecord：成功提交后的纯文本事务事实快照，用于回放与外部审计。
//!
//! `TransactionRecord` 不是编辑入口；它只是 `Buffer::edit_recorded` 成功后产生的可重放事实。
//! 重建内部事务时必然合法（已通过事务管线一次），但回放仍然走标准事务管线，不绕过任何边界校验。

use super::{EditList, Transaction, TransactionMetadata};
use crate::{
    EngineResult,
    types::{BufferVersion, TransactionId},
};

/// 一次成功提交的事务事实，包含 forward edits、inverse edits 与 metadata。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionRecord {
    transaction_id: TransactionId,
    old_version: BufferVersion,
    new_version: BufferVersion,
    edits: EditList,
    inverse_edits: EditList,
    metadata: TransactionMetadata,
}

impl TransactionRecord {
    pub(crate) fn new(
        transaction_id: TransactionId,
        old_version: BufferVersion,
        new_version: BufferVersion,
        edits: EditList,
        inverse_edits: EditList,
        metadata: TransactionMetadata,
    ) -> Self {
        Self {
            transaction_id,
            old_version,
            new_version,
            edits,
            inverse_edits,
            metadata,
        }
    }

    pub fn transaction_id(&self) -> TransactionId {
        self.transaction_id
    }

    pub fn old_version(&self) -> BufferVersion {
        self.old_version
    }

    pub fn new_version(&self) -> BufferVersion {
        self.new_version
    }

    pub fn edits(&self) -> &[crate::Edit] {
        self.edits.as_slice()
    }

    /// 重建可回放的 `Transaction`。回放后 `Buffer` 将从 `old_version` 推进到 `new_version`。
    pub(crate) fn to_transaction(&self) -> EngineResult<Transaction> {
        Ok(Transaction::new(self.old_version, self.edits.clone())?
            .with_metadata(self.metadata.clone()))
    }
}
