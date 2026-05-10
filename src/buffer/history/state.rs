//! 历史图：以单调 `HistoryNodeId` 维护节点 + parent/children 边，支撑撤销后的本地分支。
//!
//! 本文件只管理图的局部不变量；replay batches、selection 还原与版本推进由 history::api 负责。

use std::collections::BTreeMap;

use super::{HistoryEntry, HistoryNode, HistoryNodeId};

/// 历史摘要：与线性历史一致的 undo / redo 深度语义；`current_node` 暴露当前历史
/// 节点身份，便于宿主在分支查询时定位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryStatus {
    /// 当前节点到根的祖先数量；线性场景下与原 `undo_stack.len()` 一致。
    pub undo_depth: usize,
    /// 沿默认 redo 分支（最近创建子节点链）从当前节点走到叶子的节点数；
    /// 线性场景下与原 `redo_stack.len()` 一致；分支节点通过 `redo_branches()` 单独枚举。
    pub redo_depth: usize,
    /// 当前所在历史节点身份；`None` 表示位于历史树根（空 Buffer 或全部 undo 后的状态）。
    pub current_node: Option<HistoryNodeId>,
}

#[derive(Debug, Clone, Default)]
pub(in crate::buffer) struct HistoryState {
    nodes: BTreeMap<HistoryNodeId, HistoryNode>,
    roots: Vec<HistoryNodeId>,
    current: Option<HistoryNodeId>,
    next_id: u64,
    next_sequence: u64,
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

    pub(in crate::buffer) fn can_redo(&self) -> bool {
        self.children_of_current().last().is_some()
    }

    pub(in crate::buffer) fn status(&self) -> HistoryStatus {
        HistoryStatus {
            undo_depth: self.undo_depth(),
            redo_depth: self.redo_depth(),
            current_node: self.current,
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
    pub(in crate::buffer) fn push_child(&mut self, entry: HistoryEntry) -> HistoryNodeId {
        let id = HistoryNodeId::new(self.next_id);
        self.next_id = self.next_id.checked_add(1).expect("HistoryNodeId 溢出");
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("history sequence 溢出");

        let parent = self.current;
        let node = HistoryNode::new(id, sequence, parent, entry);
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
        id
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
        let merged = HistoryEntry::merge(current_node.entry.clone(), entry);
        current_node.entry = merged;
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
        if let Some(id) = self.current {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.children.clear();
            }
        }
    }

    fn drop_subtree(&mut self, root: HistoryNodeId) {
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.remove(&id) {
                stack.extend(node.children);
            }
        }
    }

    pub(in crate::buffer) fn clear(&mut self) {
        self.nodes.clear();
        self.roots.clear();
        self.current = None;
    }

    /// 按节点总数预算截断；优先丢弃序号最小的非 current 叶节点。
    pub(in crate::buffer) fn truncate_to_max_nodes(&mut self, max: usize) {
        if max == 0 {
            self.clear();
            return;
        }

        while self.nodes.len() > max {
            let Some(victim) = self.find_oldest_disposable_leaf() else {
                break;
            };
            self.detach_and_remove(victim);
        }
    }

    fn find_oldest_disposable_leaf(&self) -> Option<HistoryNodeId> {
        self.nodes
            .values()
            .filter(|node| node.children.is_empty() && Some(node.id) != self.current)
            .min_by_key(|node| node.sequence_number)
            .map(|node| node.id)
    }

    fn detach_and_remove(&mut self, id: HistoryNodeId) {
        let Some(node) = self.nodes.remove(&id) else {
            return;
        };
        match node.parent {
            Some(parent_id) => {
                if let Some(parent_node) = self.nodes.get_mut(&parent_id) {
                    parent_node.children.retain(|child| *child != id);
                }
            }
            None => self.roots.retain(|root| *root != id),
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
