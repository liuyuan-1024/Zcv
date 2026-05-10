//! 历史节点身份与图结构基础类型。
//!
//! `HistoryNodeId` 在 Buffer 生命周期内单调递增，永不复用；`HistoryNode` 以
//! parent / children 链接组织成历史树，支撑撤销后产生的本地分支。

use super::HistoryEntry;

/// 单个 Buffer 内 history node 的稳定身份；跨节点单调递增，永不回收。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HistoryNodeId(u64);

impl HistoryNodeId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone)]
pub(in crate::buffer) struct HistoryNode {
    pub(super) id: HistoryNodeId,
    pub(super) sequence_number: u64,
    pub(super) parent: Option<HistoryNodeId>,
    pub(super) children: Vec<HistoryNodeId>,
    pub(super) entry: HistoryEntry,
}

impl HistoryNode {
    pub(super) fn new(
        id: HistoryNodeId,
        sequence_number: u64,
        parent: Option<HistoryNodeId>,
        entry: HistoryEntry,
    ) -> Self {
        Self {
            id,
            sequence_number,
            parent,
            children: Vec::new(),
            entry,
        }
    }
}
