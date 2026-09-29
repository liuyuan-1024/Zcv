//! EditOperation 与 EditEvent：描述一次成功事务提交后的版本推进事实。
//!
//! EditOperation 是一次成功提交的版本推进事实；EditEvent 额外绑定事务 ID 与来源。

use super::{EditList, TransactionSource};
use crate::types::{BufferVersion, TransactionId};

/// 一次提交中已接受的编辑事实。
///
/// 坐标仍以旧文本为基准；对应 Zed `text::EditOperation` 的版本推进语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditOperation {
    /// 事务应用前的 BufferVersion。
    old_version: BufferVersion,
    /// 事务成功应用后的 BufferVersion。
    new_version: BufferVersion,
    /// 已排序、已验证的编辑列表，坐标仍以旧文本为基准。
    edits: EditList,
}

impl EditOperation {
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

    pub fn edits(&self) -> &[crate::Edit] {
        self.edits.as_slice()
    }
}

/// 文本变更事件。
///
/// 一次成功文本提交后的事务管线内部事实，承载版本与来源。
///
/// 对外发布只经 `Buffer::subscribe` 的订阅批次（携带 `PositionMap`）；它不作为宿主 API。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditEvent {
    /// 本次成功提交分配到的事务身份。
    transaction_id: TransactionId,
    /// 事务来源，用于历史观察和外部同步，不表达 Command 层语义。
    source: TransactionSource,
    /// 文本增量事实。
    operation: EditOperation,
}

impl EditEvent {
    pub(crate) fn new(
        transaction_id: TransactionId,
        source: TransactionSource,
        operation: EditOperation,
    ) -> Self {
        Self {
            transaction_id,
            source,
            operation,
        }
    }

    pub fn transaction_id(&self) -> TransactionId {
        self.transaction_id
    }

    pub fn old_version(&self) -> BufferVersion {
        self.operation.old_version()
    }

    pub fn new_version(&self) -> BufferVersion {
        self.operation.new_version()
    }

    pub fn operation(&self) -> &EditOperation {
        &self.operation
    }
}
