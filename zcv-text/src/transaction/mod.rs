//! 事务模型：定义文本变异从 Edit 到 EditEvent 的底层链路。
//!
//! 本模块负责 public 事务语义、版本绑定和变更映射，不直接访问 Buffer 存储，
//! 也不处理 UI 命令概念。

mod core;
mod edit;
mod edit_list;
mod edit_operation;
mod metadata;
mod source;

pub(crate) use core::Transaction;
pub use edit::Edit;
pub(crate) use edit_list::EditList;
pub(crate) use edit_operation::EditEvent;
pub use edit_operation::EditOperation;
pub use metadata::{TransactionMergePolicy, TransactionMetadata};
pub use source::TransactionSource;
