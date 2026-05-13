//! Buffer 增量事件队列：保存成功文本提交后的 DeltaEvent，供宿主或后续内部系统消费。
//!
//! 本文件只管理事件入队、最后事件快照和队列清空；事件生成事实来自事务提交管线。

use crate::{DeltaEvent, EngineError, EngineResult, StorageError, TransactionId};

use super::Buffer;

impl Buffer {
    pub fn pending_delta_event_count(&self) -> usize {
        self.pending_delta_events.len()
    }

    /// 不消费地查看 pending 队列；事件按提交顺序排列，可用于消费者按版本检测漏读。
    pub fn pending_delta_events(&self) -> &[DeltaEvent] {
        &self.pending_delta_events
    }

    pub fn take_pending_events(&mut self) -> Vec<DeltaEvent> {
        std::mem::take(&mut self.pending_delta_events)
    }

    pub fn last_delta_event(&self) -> Option<&DeltaEvent> {
        self.last_delta_event.as_ref()
    }

    pub(in crate::buffer) fn prepare_transaction_id(
        &self,
    ) -> EngineResult<(TransactionId, TransactionId)> {
        let transaction_id = self.next_transaction_id;
        let next_transaction_id = transaction_id
            .next()
            .ok_or(EngineError::TransactionIdOverflow)?;
        Ok((transaction_id, next_transaction_id))
    }

    pub(in crate::buffer) fn reserve_delta_event_slot(&mut self) -> EngineResult<()> {
        self.pending_delta_events
            .try_reserve(1)
            .map_err(|_| StorageError::OutOfMemory)?;
        Ok(())
    }

    pub(in crate::buffer) fn commit_delta_event(
        &mut self,
        next_transaction_id: TransactionId,
        last_event: DeltaEvent,
        pending_event: DeltaEvent,
    ) {
        self.next_transaction_id = next_transaction_id;
        self.last_delta_event = Some(last_event);
        self.pending_delta_events.push(pending_event);
    }
}
