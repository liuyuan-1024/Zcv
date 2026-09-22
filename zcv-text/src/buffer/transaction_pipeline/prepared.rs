//! PreparedTransaction / DerivedBufferState：事务管线阶段之间传递的内部事实。
//!
//! 本文件不暴露 public API，也不执行任何文本变异。

use crate::{
    buffer::history::{HistoryState, TransactionSession},
    storage::RopeyStorage,
    tracking::{CoordinateIndex, EditLog, InsertionIndex},
    transaction::{DeltaEvent, EditList, TransactionMetadata},
    types::{BufferVersion, TransactionId},
};

pub(in crate::buffer) struct PreparedTransaction {
    pub(in crate::buffer) edits: EditList,
    pub(in crate::buffer) metadata: TransactionMetadata,
    /// 逆编辑；进入历史时写入编辑日志，供 undo 回放。
    pub(in crate::buffer) undo_edits: EditList,
}

/// 一次事务在克隆状态上计划出的完整结果；安装时整体换入 Buffer。
///
/// 对齐 Zed 的 EditedBufferSnapshot：规划阶段不触碰主文档，安装阶段只做 move 换入与订阅发布，
/// 因此不再需要先提交文本、再重放派生编辑。
pub(in crate::buffer) struct DerivedBufferState {
    pub(in crate::buffer) storage: RopeyStorage,
    pub(in crate::buffer) version: BufferVersion,
    pub(in crate::buffer) edit_log: EditLog,
    pub(in crate::buffer) coordinate_index: CoordinateIndex,
    pub(in crate::buffer) insertions: InsertionIndex,
    pub(in crate::buffer) history: HistoryState,
    pub(in crate::buffer) session: Option<TransactionSession>,
    pub(in crate::buffer) next_transaction_id: TransactionId,
    pub(in crate::buffer) event: DeltaEvent,
    pub(in crate::buffer) history_transaction_id: Option<TransactionId>,
}
