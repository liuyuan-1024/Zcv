//! PreparedTransaction：事务提交前已经验证并补齐的内部工作包。
//!
//! 本文件只承载管线阶段之间传递的事实，不暴露 public API，也不执行任何文本变异。

use crate::{
    BufferVersion,
    transaction::{EditList, TransactionMetadata},
};

pub(in crate::buffer) struct PreparedTransaction {
    pub(in crate::buffer) base_version: BufferVersion,
    pub(in crate::buffer) edits: EditList,
    pub(in crate::buffer) metadata: TransactionMetadata,
    pub(in crate::buffer) undo_edits: EditList,
    pub(in crate::buffer) redo_edits: EditList,
}
