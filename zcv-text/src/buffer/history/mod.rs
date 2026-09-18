//! Undo / Redo 历史子系统。
//!
//! # Invariants
//! - 历史节点只保存版本区间、事务身份与描述，不复制可重放编辑。
//! - 节点通过 parent/children 组成历史图；撤销后产生的新编辑形成本地分支，不删除其他 redo 分支。
//! - merge 只修改当前历史节点，并保持该节点的父子关系与规范事务身份稳定。

mod api;
mod entry;
mod node;
mod session;
mod state;

pub(in crate::buffer) use session::TransactionSession;

pub use api::HistoryEditOutcome;

pub(in crate::buffer) use entry::HistoryEntry;
pub(in crate::buffer) use node::{HistoryNode, HistoryNodeId};
pub(in crate::buffer) use state::HistoryState;
