//! Delta 与 DeltaEvent：描述一次成功事务提交后的版本推进事实。
//!
//! Delta 只携带文本增量；DeltaEvent 额外绑定事务 ID、来源、ChangeSet 和 PositionMap。

use crate::{
    position_map::PositionMap,
    types::{BufferVersion, TextRange, TransactionId},
    versioned::VersionedResult,
};

use super::{ChangeSet, EditList, TransactionSource};

/// 增量事件，事务提交后生成。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    /// 事务应用前的 BufferVersion。
    old_version: BufferVersion,
    /// 事务成功应用后的 BufferVersion。
    new_version: BufferVersion,
    /// 已排序、已验证的编辑列表，坐标仍以旧文本为基准。
    edits: EditList,
}

impl Delta {
    pub(crate) fn new(
        old_version: BufferVersion,
        new_version: BufferVersion,
        edits: EditList,
    ) -> Self {
        Self {
            old_version,
            new_version,
            edits,
        }
    }

    pub fn old_version(&self) -> BufferVersion {
        self.old_version
    }

    pub fn new_version(&self) -> BufferVersion {
        self.new_version
    }

    pub fn edits(&self) -> &EditList {
        &self.edits
    }
}

/// 文本变更事件。
///
/// `DeltaEvent` 是一次成功文本提交后的可消费事实，供后续 Anchor、
/// TrackedRange、metadata layer、外部分析结果等统一感知版本推进和位置映射。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaEvent {
    /// 本次成功提交分配到的事务身份。
    transaction_id: TransactionId,
    /// 事件对应的旧 BufferVersion。
    old_version: BufferVersion,
    /// 事件对应的新 BufferVersion。
    new_version: BufferVersion,
    /// 事务来源，用于历史观察和外部同步，不表达 Command 层语义。
    source: TransactionSource,
    /// 文本增量事实。
    delta: Delta,
    /// 可查询 changed ranges 的已验证变更集合。
    changeset: ChangeSet,
    /// old -> new / new -> old 坐标映射器，供 Anchor、TrackedRange 和宿主复用。
    position_map: PositionMap,
}

impl DeltaEvent {
    pub(crate) fn new(
        transaction_id: TransactionId,
        source: TransactionSource,
        delta: Delta,
        changeset: ChangeSet,
        position_map: PositionMap,
    ) -> Self {
        Self {
            transaction_id,
            old_version: delta.old_version(),
            new_version: delta.new_version(),
            source,
            delta,
            changeset,
            position_map,
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

    pub fn source(&self) -> TransactionSource {
        self.source
    }

    pub fn delta(&self) -> &Delta {
        &self.delta
    }

    pub fn changeset(&self) -> &ChangeSet {
        &self.changeset
    }

    pub fn position_map(&self) -> &PositionMap {
        &self.position_map
    }

    /// 在新版本上的 changed ranges 只读结果，已绑定 `new_version` 供宿主版本对齐。
    pub fn changed_ranges_result(&self) -> VersionedResult<Vec<TextRange>> {
        VersionedResult::new(self.new_version, self.changeset.changed_ranges())
    }
}
