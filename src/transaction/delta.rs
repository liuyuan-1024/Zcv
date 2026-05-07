//! Delta 与 DeltaEvent：描述一次成功事务提交后的版本推进事实。
//!
//! Delta 只携带文本增量；DeltaEvent 额外绑定事务 ID、来源、ChangeSet 和 PositionMap。

use crate::{
    position_map::PositionMap,
    types::{BufferVersion, TransactionId},
};

use super::{ChangeSet, EditList, TransactionSource};

/// 增量事件，事务提交后生成。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    /// 事务应用前的 BufferVersion。
    pub old_version: BufferVersion,
    /// 事务成功应用后的 BufferVersion。
    pub new_version: BufferVersion,
    /// 已排序、已验证的编辑列表，坐标仍以旧文本为基准。
    pub edits: EditList,
}

/// 文本变更事件。
///
/// `DeltaEvent` 是一次成功文本提交后的可消费事实，供后续 Anchor、
/// TrackedRange、metadata layer、外部分析结果等统一感知版本推进和位置映射。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaEvent {
    /// 本次成功提交分配到的事务身份。
    pub transaction_id: TransactionId,
    /// 事件对应的旧 BufferVersion。
    pub old_version: BufferVersion,
    /// 事件对应的新 BufferVersion。
    pub new_version: BufferVersion,
    /// 事务来源，用于历史观察和外部同步，不表达 Command 层语义。
    pub source: TransactionSource,
    /// 文本增量事实。
    pub delta: Delta,
    /// 可查询 changed ranges 的已验证变更集合。
    pub changeset: ChangeSet,
    /// old -> new / new -> old 坐标映射器，供 Anchor、TrackedRange 和宿主复用。
    pub position_map: PositionMap,
}
