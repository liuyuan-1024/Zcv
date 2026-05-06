//! Buffer 增量事件队列：保存成功文本提交后的 DeltaEvent，供宿主或后续内部系统消费。
//!
//! 本文件只管理事件入队、最后事件快照和队列清空；事件生成事实来自事务提交管线。

use crate::{DeltaEvent, EngineError, EngineResult, TransactionId};

use super::Buffer;

impl Buffer {
    pub fn pending_delta_event_count(&self) -> usize {
        self.pending_delta_events.len()
    }

    pub fn take_pending_events(&mut self) -> Vec<DeltaEvent> {
        std::mem::take(&mut self.pending_delta_events)
    }

    pub fn last_delta_event(&self) -> Option<&DeltaEvent> {
        self.last_delta_event.as_ref()
    }

    pub(in crate::buffer) fn reserve_transaction_id(&mut self) -> EngineResult<TransactionId> {
        let transaction_id = self.next_transaction_id;
        self.next_transaction_id = self
            .next_transaction_id
            .next()
            .ok_or(EngineError::TransactionIdOverflow)?;
        Ok(transaction_id)
    }

    pub(in crate::buffer) fn push_delta_event(&mut self, event: DeltaEvent) {
        self.last_delta_event = Some(event.clone());
        self.pending_delta_events.push(event);
    }
}
