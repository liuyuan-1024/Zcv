//! HistoryEntry 数据边界：描述一次可撤销纯文本历史节点所需的版本区间与身份。
//!
//! 节点不再复制可重放编辑；具体编辑事实由 `tracking::EditLog` 按版本区间提供。

use std::sync::Arc;

use crate::{BufferVersion, TransactionId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::buffer) struct HistoryEntry {
    pub(in crate::buffer) transaction_id: TransactionId,
    /// 本事务开始前的 BufferVersion。
    pub(in crate::buffer) start_version: BufferVersion,
    /// 本事务最后一次提交后的 BufferVersion。
    pub(in crate::buffer) end_version: BufferVersion,
    pub(in crate::buffer) description: Option<Arc<str>>,
}

impl HistoryEntry {
    pub(in crate::buffer) fn new(
        transaction_id: TransactionId,
        start_version: BufferVersion,
        end_version: BufferVersion,
        description: Option<Arc<str>>,
    ) -> Self {
        Self {
            transaction_id,
            start_version,
            end_version,
            description,
        }
    }

    /// 把后一个事务合并进前一个节点：身份与起点保持，终点与描述取后一次。
    pub(in crate::buffer) fn merge(previous: Self, next: Self) -> Self {
        Self {
            transaction_id: previous.transaction_id,
            start_version: previous.start_version,
            end_version: next.end_version,
            description: next.description.or(previous.description),
        }
    }
}
