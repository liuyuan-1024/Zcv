//! Buffer 本地编辑入口：一次接收完整编辑批次并进入事务与历史管线。

use crate::buffer::Buffer;
use crate::{
    TextResult,
    transaction::{Edit, Transaction, TransactionMetadata},
};

impl Buffer {
    /// 应用一个本地编辑批次。所有编辑共享同一事务身份、历史策略与版本推进。
    ///
    /// 提交结果由 `Buffer::subscribe` 的订阅批次发布；调用方不需要接收返回值。
    pub fn edit(
        &mut self,
        edits: impl IntoIterator<Item = Edit>,
        metadata: TransactionMetadata,
    ) -> TextResult<()> {
        let transaction = Transaction::from_edits(self.version, edits.into_iter().collect())?
            .with_metadata(metadata);
        self.apply_transaction(transaction)
    }
}
