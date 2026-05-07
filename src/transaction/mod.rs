//! 事务模型：定义文本变异从 Edit 到 DeltaEvent 的底层链路。
//!
//! 本模块负责 public 事务语义、版本绑定和变更映射，不直接访问 Buffer 存储，
//! 也不处理 UI 命令概念。

mod changeset;
mod delta;
mod edit;
mod edit_list;
mod metadata;
mod record;
mod source;

pub use changeset::ChangeSet;
pub use delta::{Delta, DeltaEvent};
pub use edit::Edit;
pub use edit_list::EditList;
pub use metadata::{TransactionMergePolicy, TransactionMetadata};
pub use record::Transaction;
pub use source::TransactionSource;
