//! Buffer 本地编辑入口：一次接收完整编辑批次并进入事务与历史管线。

use crate::buffer::Buffer;
use crate::{
    EngineResult,
    transaction::{Edit, Transaction, TransactionMetadata, TransactionOutcome},
};

impl Buffer {
    /// 应用一个本地编辑批次。所有编辑共享同一事务身份、历史策略与版本推进。
    pub fn edit(
        &mut self,
        edits: impl IntoIterator<Item = Edit>,
        metadata: TransactionMetadata,
    ) -> EngineResult<TransactionOutcome> {
        let transaction = Transaction::from_edits(self.version, edits.into_iter().collect())?
            .with_metadata(metadata);
        self.apply_transaction(transaction)
    }

    /// 应用一个本地编辑批次，并返回可供外部同步或回放的事务记录。
    pub fn edit_recorded(
        &mut self,
        edits: impl IntoIterator<Item = Edit>,
        metadata: TransactionMetadata,
    ) -> EngineResult<crate::TransactionRecord> {
        let transaction = Transaction::from_edits(self.version, edits.into_iter().collect())?
            .with_metadata(metadata);
        self.apply_transaction_recorded(transaction)
    }
}
