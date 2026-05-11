//! 历史节点身份与图结构基础类型。
//!
//! `HistoryNodeId` 在 Buffer 生命周期内单调递增，永不复用；`HistoryNode` 以
//! parent / children 链接组织成历史树，支撑撤销后产生的本地分支。

use super::HistoryEntry;

/// 单个 Buffer 内 history node 的稳定身份；跨节点单调递增，永不回收。
///
/// FFI 友好：`#[repr(transparent)]` 让宿主跨 FFI 直接当 `uint64_t` 使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct HistoryNodeId(u64);

impl HistoryNodeId {
    pub(crate) const fn new(value: u64) -> Self {
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
    /// 缓存的 `entry.byte_size()`；构造时与 entry 替换 / 合并后必须同步刷新。
    pub(super) entry_bytes: usize,
}

impl HistoryNode {
    pub(super) fn new(
        id: HistoryNodeId,
        sequence_number: u64,
        parent: Option<HistoryNodeId>,
        entry: HistoryEntry,
    ) -> Self {
        let entry_bytes = entry.byte_size();
        Self {
            id,
            sequence_number,
            parent,
            children: Vec::new(),
            entry,
            entry_bytes,
        }
    }

    pub(super) fn replace_entry(&mut self, entry: HistoryEntry) {
        self.entry_bytes = entry.byte_size();
        self.entry = entry;
    }
}
