//! Buffer 文本变化订阅与事务身份推进。

use super::Buffer;
use crate::{
    errors::{TextError, TextResult},
    text_changes::{TextPatch, TextSubscription},
    transaction::DeltaEvent,
    types::TransactionId,
};

impl Buffer {
    pub fn subscribe(&self) -> TextSubscription {
        self.text_changes.subscribe(self.version)
    }

    pub(in crate::buffer) fn prepare_transaction_id(
        &self,
    ) -> TextResult<(TransactionId, TransactionId)> {
        let transaction_id = self.next_transaction_id;
        let next_transaction_id = transaction_id
            .next()
            .ok_or(TextError::TransactionIdOverflow)?;
        Ok((transaction_id, next_transaction_id))
    }

    pub(in crate::buffer) fn commit_delta_event(
        &mut self,
        next_transaction_id: TransactionId,
        event: &DeltaEvent,
    ) {
        self.next_transaction_id = next_transaction_id;
        let patch = TextPatch::from_delta(event.delta());
        self.text_changes
            .publish(event.old_version(), event.new_version(), patch, false);
    }
}
