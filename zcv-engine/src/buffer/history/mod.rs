//! Undo / Redo 历史子系统。
//!
//! # Invariants
//! - 历史节点只保存可重放的最小编辑批次，不保存全文快照。
//! - 任意 `HistoryEntry` 至少包含一组 `undo_batches` 与 `redo_batches`。
//! - 节点通过 parent/children 组成历史图；撤销后产生的新编辑形成本地分支，不删除其他 redo 分支。
//! - merge 只修改当前历史节点，并保持该节点的父子关系与规范事务身份稳定。

mod api;
mod entry;
mod node;
mod state;

pub use api::{HistoryEditOutcome, HistoryNodeView};
pub use node::HistoryNodeId;
pub use state::HistoryStatus;

pub(in crate::buffer) use entry::HistoryEntry;
pub(in crate::buffer) use node::HistoryNode;
pub(in crate::buffer) use state::HistoryState;
