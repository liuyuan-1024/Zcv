//! Undo / Redo 历史子系统。
//!
//! # Invariants
//! - `undo_stack` 与 `redo_stack` 只保存可重放的最小编辑批次，不保存全文快照。
//! - 任意 `HistoryEntry` 至少包含一组 `undo_batches` 与 `redo_batches`。
//! - 新的可记录文本变更会清空 `redo_stack`，防止分支历史悬挂。
//! - merge 只在 `redo_stack` 为空时发生，保持线性历史语义。

mod api;
mod entry;
mod node;
mod state;

pub use api::HistoryNodeView;
pub use node::HistoryNodeId;
pub use state::HistoryStatus;

pub(in crate::buffer) use entry::HistoryEntry;
pub(in crate::buffer) use node::HistoryNode;
pub(in crate::buffer) use state::HistoryState;
