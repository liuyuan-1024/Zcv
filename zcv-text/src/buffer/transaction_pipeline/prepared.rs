//! PreparedTransaction：事务提交前已经验证并补齐的内部工作包。
//!
//! 本文件只承载管线阶段之间传递的事实，不暴露 public API，也不执行任何文本变异。

use crate::transaction::{EditList, TransactionMetadata};

pub(in crate::buffer) struct PreparedTransaction {
    pub(in crate::buffer) edits: EditList,
    pub(in crate::buffer) metadata: TransactionMetadata,
    /// 逆编辑；进入历史时写入编辑日志，供 undo 回放。
    pub(in crate::buffer) undo_edits: EditList,
}
