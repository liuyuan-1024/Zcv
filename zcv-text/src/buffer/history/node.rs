//! 历史节点身份与图结构基础类型。
//!
//! `HistoryNodeId` 在 Buffer 生命周期内单调递增，永不复用；`HistoryNode` 以
//! parent / children 链接组织成历史树，支撑撤销后产生的本地分支。

use super::HistoryEntry;

/// 单个 Buffer 内 history node 的稳定身份；跨节点单调递增，永不回收。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::buffer) struct HistoryNodeId(u64);

impl HistoryNodeId {
    pub(super) const fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone)]
pub(in crate::buffer) struct HistoryNode {
    pub(super) id: HistoryNodeId,
    pub(super) parent: Option<HistoryNodeId>,
    pub(super) children: Vec<HistoryNodeId>,
    pub(super) entry: HistoryEntry,
}

impl HistoryNode {
    pub(super) fn new(
        id: HistoryNodeId,
        parent: Option<HistoryNodeId>,
        entry: HistoryEntry,
    ) -> Self {
        Self {
            id,
            parent,
            children: Vec::new(),
            entry,
        }
    }

    pub(super) fn replace_entry(&mut self, entry: HistoryEntry) {
        self.entry = entry;
    }
}
