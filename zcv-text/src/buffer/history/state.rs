//! 历史图：以单调 `HistoryNodeId` 维护节点 + parent/children 边，支撑撤销后的本地分支。
//!
//! 节点只保存版本区间与事务身份；可重放编辑由 `tracking::EditLog` 按版本区间提供。

use std::collections::BTreeMap;

use super::{HistoryEntry, HistoryNode, HistoryNodeId};
use crate::{BufferVersion, TextError, TextResult};

#[derive(Debug, Clone, Default)]
pub(in crate::buffer) struct HistoryState {
    nodes: BTreeMap<HistoryNodeId, HistoryNode>,
    roots: Vec<HistoryNodeId>,
    current: Option<HistoryNodeId>,
    next_id: u64,
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

    /// 当前历史节点的事务身份；历史被预算清空时为 `None`。
    pub(in crate::buffer) fn current_transaction_id(&self) -> Option<crate::TransactionId> {
        self.current
            .and_then(|id| self.nodes.get(&id))
            .map(|node| node.entry.transaction_id)
    }

    pub(in crate::buffer) fn can_redo(&self) -> bool {
        self.children_of_current().last().is_some()
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
    ) -> TextResult<HistoryNodeId> {
        let id = HistoryNodeId::new(self.next_id);
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or(TextError::HistoryIdExhausted)?;
        self.next_id = next_id;

        let parent = self.current;
        let node = HistoryNode::new(id, parent, entry);
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

    /// 把 `entry` 的版本区间合并到当前节点（用于 `MergeWithPrevious`），仅在当前节点没有子节点时允许。
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

    /// 把 current 移到指定子节点；调用方保证目标节点是当前节点的子节点之一。
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
                stack.extend(node.children);
            }
        }
    }

    pub(in crate::buffer) fn clear(&mut self) {
        self.nodes.clear();
        self.roots.clear();
        self.current = None;
    }

    /// 按节点数预算裁剪最老的非 current 节点。
    ///
    /// current 节点保留：撤销位置本身是有效事实，预算只限制可回溯的深度。
    pub(in crate::buffer) fn truncate_to_node_budget(&mut self, max_nodes: usize) {
        if max_nodes == 0 {
            self.clear();
            return;
        }
        while self.nodes.len() > max_nodes {
            let Some(victim) = self
                .nodes
                .iter()
                .find(|(_, node)| Some(node.id) != self.current)
                .map(|(id, _)| *id)
            else {
                break;
            };
            self.splice_out_and_remove(victim);
        }
    }

    /// 丢弃起点早于 `earliest` 的历史节点，使它们与编辑日志的保留窗口一致。
    ///
    /// `None` 表示日志已清空，历史随之清空。current 节点也参与裁剪：
    /// 逆编辑已不在日志中时，它本来就无法回放。
    pub(in crate::buffer) fn retain_versions_since(&mut self, earliest: Option<BufferVersion>) {
        let Some(earliest) = earliest else {
            self.clear();
            return;
        };
        loop {
            let victim = self
                .nodes
                .iter()
                .find(|(_, node)| node.entry.start_version < earliest)
                .map(|(id, _)| *id);
            let Some(victim) = victim else {
                break;
            };
            self.splice_out_and_remove(victim);
        }
    }

    /// 把 `id` 从图中移除，并把它的子节点 splice 到 `id` 原父节点（或 roots）的
    /// 同一位置，保留兄弟节点的相对顺序。
    fn splice_out_and_remove(&mut self, id: HistoryNodeId) {
        let Some(node) = self.nodes.remove(&id) else {
            return;
        };
        let children = node.children;
        let parent = node.parent;

        if self.current == Some(id) {
            self.current = parent;
        }

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
