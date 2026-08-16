//! 历史图：以单调 `HistoryNodeId` 维护节点 + parent/children 边，支撑撤销后的本地分支。
//!
//! 本文件只管理图的局部不变量；replay batches 与版本推进由 history::api 负责。

use std::collections::BTreeMap;

use super::{HistoryEntry, HistoryNode, HistoryNodeId};
use crate::{EngineError, EngineResult, TransactionId};

/// 历史摘要：与线性历史一致的 undo / redo 深度语义；`current_node` 暴露当前历史
/// 节点身份；`node_count` / `memory_bytes` 暴露当前历史预算占用，便于宿主观测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryStatus {
    /// 当前节点到根的祖先数量；线性场景下与原 `undo_stack.len()` 一致。
    pub undo_depth: usize,
    /// 沿默认 redo 分支（最近创建子节点链）从当前节点走到叶子的节点数；
    /// 线性场景下与原 `redo_stack.len()` 一致；分支节点通过 `redo_branches()` 单独枚举。
    pub redo_depth: usize,
    /// 当前所在历史节点身份；`None` 表示位于历史树根（空 Buffer 或全部 undo 后的状态）。
    pub current_node: Option<HistoryNodeId>,
    /// 历史图当前持有的节点总数（含所有分支）。
    pub node_count: usize,
    /// 历史图按 `HistoryEntry::byte_size` 累加的字节占用估算。
    pub memory_bytes: usize,
}

#[derive(Debug, Clone, Default)]
pub(in crate::buffer) struct HistoryState {
    nodes: BTreeMap<HistoryNodeId, HistoryNode>,
    roots: Vec<HistoryNodeId>,
    current: Option<HistoryNodeId>,
    next_id: u64,
    /// 所有节点 `entry_bytes` 的累计和；预算截断时避免每轮全量重算。
    total_bytes: usize,
}

impl HistoryState {
    pub(in crate::buffer) fn new() -> Self {
        Self::default()
    }

    pub(in crate::buffer) fn current(&self) -> Option<HistoryNodeId> {
        self.current
    }

    pub(in crate::buffer) fn node(&self, id: HistoryNodeId) -> Option<&HistoryNode> {
        self.nodes.get(&id)
    }

    pub(in crate::buffer) fn can_undo(&self) -> bool {
        self.current.is_some()
    }

    /// 当前历史节点的事务身份；历史被预算清空（如 `max_undo_history=0`）时为 `None`。
    pub(in crate::buffer) fn current_transaction_id(&self) -> Option<TransactionId> {
        self.current
            .and_then(|id| self.nodes.get(&id))
            .map(|node| node.entry.transaction_id)
    }

    pub(in crate::buffer) fn can_redo(&self) -> bool {
        self.children_of_current().last().is_some()
    }

    pub(in crate::buffer) fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub(in crate::buffer) fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub(in crate::buffer) fn status(&self) -> HistoryStatus {
        HistoryStatus {
            undo_depth: self.undo_depth(),
            redo_depth: self.redo_depth(),
            current_node: self.current,
            node_count: self.node_count(),
            memory_bytes: self.total_bytes(),
        }
    }

    /// 当前节点的父节点；用于 undo 时定位。
    pub(in crate::buffer) fn parent_of_current(&self) -> Option<HistoryNodeId> {
        self.current
            .and_then(|id| self.nodes.get(&id))
            .and_then(|node| node.parent)
    }

    /// 当前可选 redo 分支，按创建顺序排列（末尾为最近一次创建的子节点 = 默认 redo 目标）。
    pub(in crate::buffer) fn children_of_current(&self) -> &[HistoryNodeId] {
        match self.current {
            Some(id) => self
                .nodes
                .get(&id)
                .map(|node| node.children.as_slice())
                .unwrap_or(&[]),
            None => &self.roots,
        }
    }

    /// 把 `entry` 作为当前节点的新子节点入图，并把 current 移到新节点。
    pub(in crate::buffer) fn push_child(
        &mut self,
        entry: HistoryEntry,
    ) -> EngineResult<HistoryNodeId> {
        let id = HistoryNodeId::new(self.next_id);
        let sequence = self.next_id;
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or(EngineError::HistoryIdExhausted)?;
        self.next_id = next_id;

        let parent = self.current;
        let node = HistoryNode::new(id, sequence, parent, entry);
        self.total_bytes += node.entry_bytes;
        self.nodes.insert(id, node);

        match parent {
            Some(parent_id) => {
                if let Some(parent_node) = self.nodes.get_mut(&parent_id) {
                    parent_node.children.push(id);
                }
            }
            None => self.roots.push(id),
        }

        self.current = Some(id);
        Ok(id)
    }

    /// 把 `entry` 的批次合并到当前节点（用于 `MergeWithPrevious`），仅在当前节点没有子节点时允许。
    pub(in crate::buffer) fn merge_into_current(&mut self, entry: HistoryEntry) -> bool {
        let Some(current_id) = self.current else {
            return false;
        };
        let Some(current_node) = self.nodes.get_mut(&current_id) else {
            return false;
        };
        if !current_node.children.is_empty() {
            return false;
        }
        let old_bytes = current_node.entry_bytes;
        let merged = HistoryEntry::merge(current_node.entry.clone(), entry);
        self.total_bytes = self.total_bytes - old_bytes + merged.byte_size();
        current_node.replace_entry(merged);
        true
    }

    /// undo：把 current 移到当前节点的父节点，返回原 current 节点引用。
    pub(in crate::buffer) fn step_undo(&mut self) -> Option<&HistoryNode> {
        let leaving_id = self.current?;
        let parent = self.nodes.get(&leaving_id)?.parent;
        self.current = parent;
        self.nodes.get(&leaving_id)
    }

    /// 默认 redo：选择当前节点 / 根集合中最近创建的子节点。
    pub(in crate::buffer) fn default_redo_target(&self) -> Option<HistoryNodeId> {
        self.children_of_current().last().copied()
    }

    /// 把 current 移到指定子节点；调用方保证 `child_id` 是 `children_of_current()` 之一。
    pub(in crate::buffer) fn step_redo_into(
        &mut self,
        child_id: HistoryNodeId,
    ) -> Option<&HistoryNode> {
        if !self.children_of_current().contains(&child_id) {
            return None;
        }
        self.current = Some(child_id);
        self.nodes.get(&child_id)
    }

    /// 删除当前节点的所有子节点子树（含递归）。配合 `record_history=false` 的提交语义。
    pub(in crate::buffer) fn drop_children_of_current(&mut self) {
        let children: Vec<HistoryNodeId> = match self.current {
            Some(id) => self
                .nodes
                .get(&id)
                .map(|node| node.children.clone())
                .unwrap_or_default(),
            None => std::mem::take(&mut self.roots),
        };
        for child in children {
            self.drop_subtree(child);
        }
        if let Some(id) = self.current
            && let Some(node) = self.nodes.get_mut(&id)
        {
            node.children.clear();
        }
    }

    fn drop_subtree(&mut self, root: HistoryNodeId) {
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.remove(&id) {
                self.total_bytes -= node.entry_bytes;
                stack.extend(node.children);
            }
        }
    }

    pub(in crate::buffer) fn clear(&mut self) {
        self.nodes.clear();
        self.roots.clear();
        self.current = None;
        self.total_bytes = 0;
    }

    /// 双预算截断：节点数上限 + 历史字节上限。
    ///
    /// - `max_nodes == 0` 直接清空整个历史图。
    /// - `max_bytes == 0` 表示不限制字节预算，仅按节点计数截断。
    /// - 截断按 sequence_number 从最老的非 current 节点开始丢弃；被丢弃节点的
    ///   子节点被 splice 到原父节点（或 roots）的同一位置，保持图连通。
    /// - current 节点永远保留：即使仅存它一个时仍超字节预算也不丢（防止丢失编辑
    ///   位置事实，调用方应通过 `set_large_file_policy` 选择更宽预算）。
    pub(in crate::buffer) fn truncate_to_budget(&mut self, max_nodes: usize, max_bytes: usize) {
        if max_nodes == 0 {
            self.clear();
            return;
        }

        // 预算内的常见路径 O(1) 早退：字节计数已缓存，不再每轮全量求和。
        loop {
            let over_count = self.nodes.len() > max_nodes;
            let over_bytes = max_bytes != 0 && self.total_bytes > max_bytes;
            if !over_count && !over_bytes {
                break;
            }
            let Some(victim) = self.find_oldest_disposable() else {
                break;
            };
            self.splice_out_and_remove(victim);
        }
    }

    /// 最老的可淘汰节点：BTreeMap 按节点 id 升序（id 即创建序号），从最小 id 起找到的第一个非 current 节点即最老可淘汰者。
    fn find_oldest_disposable(&self) -> Option<HistoryNodeId> {
        self.nodes
            .iter()
            .find(|(_, node)| Some(node.id) != self.current)
            .map(|(id, _)| *id)
    }

    /// 把 `id` 从图中移除，并把它的子节点 splice 到 `id` 原父节点（或 roots）的
    /// 同一位置，保留兄弟节点的相对顺序。
    fn splice_out_and_remove(&mut self, id: HistoryNodeId) {
        let Some(node) = self.nodes.remove(&id) else {
            return;
        };
        self.total_bytes -= node.entry_bytes;
        let children = node.children;
        let parent = node.parent;

        for child_id in &children {
            if let Some(child) = self.nodes.get_mut(child_id) {
                child.parent = parent;
            }
        }

        match parent {
            Some(parent_id) => {
                if let Some(parent_node) = self.nodes.get_mut(&parent_id) {
                    splice_children(&mut parent_node.children, id, &children);
                }
            }
            None => splice_children(&mut self.roots, id, &children),
        }
    }

    fn undo_depth(&self) -> usize {
        let mut depth = 0;
        let mut cursor = self.current;
        while let Some(id) = cursor {
            depth += 1;
            cursor = self.nodes.get(&id).and_then(|node| node.parent);
        }
        depth
    }

    fn redo_depth(&self) -> usize {
        let mut depth = 0;
        let mut cursor = self.current;
        loop {
            let next = match cursor {
                Some(id) => self
                    .nodes
                    .get(&id)
                    .and_then(|node| node.children.last().copied()),
                None => self.roots.last().copied(),
            };
            match next {
                Some(next_id) => {
                    depth += 1;
                    cursor = Some(next_id);
                }
                None => break,
            }
        }
        depth
    }
}

/// 把 `list` 中的 `victim` 替换为 `replacements`，保留 `victim` 原位置以维持兄弟顺序。
/// 若 `victim` 不在 `list`（理论上不应发生），保持 `list` 不变。
fn splice_children(
    list: &mut Vec<HistoryNodeId>,
    victim: HistoryNodeId,
    replacements: &[HistoryNodeId],
) {
    let Some(pos) = list.iter().position(|id| *id == victim) else {
        return;
    };
    list.splice(pos..=pos, replacements.iter().copied());
}
