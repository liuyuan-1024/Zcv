//! History public API：把 Undo / Redo + 本地分支能力暴露为 Buffer 方法。
//!
//! 本文件负责历史图的 cursor 移动、分支查询与重放，不定义 HistoryEntry 的存储形态。

use std::sync::Arc;

use crate::{
    EngineError, EngineResult, LargeFilePolicy, TransactionId, TransactionSource,
    buffer::Buffer,
    transaction::{ChangeSet, Delta, Edit, EditList, TransactionMergePolicy, TransactionMetadata},
};

use super::{HistoryEntry, HistoryNodeId, HistoryStatus};

/// 当前历史节点的只读视图，用于宿主感知节点身份和分支结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryNodeView {
    /// 节点稳定身份。
    pub id: HistoryNodeId,
    /// 节点创建时分配的单调序号，跨 Buffer 寿命永不复用。
    pub sequence_number: u64,
    /// 父节点身份；`None` 表示该节点是历史树的根（首次提交后的节点）。
    pub parent: Option<HistoryNodeId>,
    /// 子节点身份列表，按创建时间顺序排列，末尾为最近一次创建（默认 redo 目标）。
    pub children: Vec<HistoryNodeId>,
    /// 宿主用于关联视图状态历史的规范事务身份。
    pub transaction_id: TransactionId,
    /// 节点描述（来自 `TransactionMetadata::description`）。
    /// `Arc<str>` 让 host 端 clone 这个 view 时仍是 O(1) 引用计数。
    pub description: Option<Arc<str>>,
}

/// 一次 Undo / Redo 文本回放的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEditOutcome {
    transaction_id: TransactionId,
    delta: Delta,
    changeset: ChangeSet,
}

impl HistoryEditOutcome {
    fn new(transaction_id: TransactionId, delta: Delta, changeset: ChangeSet) -> Self {
        Self {
            transaction_id,
            delta,
            changeset,
        }
    }

    /// 被回放历史节点的规范事务身份。
    pub fn transaction_id(&self) -> TransactionId {
        self.transaction_id
    }

    pub fn delta(&self) -> &Delta {
        &self.delta
    }

    pub fn changeset(&self) -> &ChangeSet {
        &self.changeset
    }
}

impl Buffer {
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn history_status(&self) -> HistoryStatus {
        self.history.status()
    }

    /// 当前所在历史节点；`None` 表示历史树根（空 Buffer 或全部 undo 后的状态）。
    pub fn current_history_node(&self) -> Option<HistoryNodeId> {
        self.history.current()
    }

    /// 读取指定节点的只读视图。
    pub fn history_node(&self, id: HistoryNodeId) -> Option<HistoryNodeView> {
        self.history.node(id).map(|node| HistoryNodeView {
            id: node.id,
            sequence_number: node.sequence_number,
            parent: node.parent,
            children: node.children.clone(),
            transaction_id: node.entry.transaction_id,
            description: node.entry.description.clone(),
        })
    }

    /// 当前节点的 undo 目标节点身份（即父节点）；`None` 表示已经在历史根。
    pub fn parent_history_node(&self) -> Option<HistoryNodeId> {
        self.history.parent_of_current()
    }

    /// 当前节点可选 redo 分支（即子节点列表），按最近创建优先排列。
    pub fn redo_branches(&self) -> Vec<HistoryNodeId> {
        let mut children = self.history.children_of_current().to_vec();
        children.reverse();
        children
    }

    /// 撤销最近一次历史节点。
    ///
    /// 没有可撤销历史时返回 `Ok(None)`，避免把空历史当作错误。
    pub fn undo(&mut self) -> EngineResult<Option<HistoryEditOutcome>> {
        self.ensure_writable()?;

        let undo_target = {
            let Some(node_id) = self.history.current() else {
                return Ok(None);
            };
            let node = self
                .history
                .node(node_id)
                .ok_or_else(|| EngineError::EngineBug {
                    location: "Buffer::undo",
                    detail: format!("当前历史节点 {node_id:?} 缺失"),
                })?;
            if node.entry.undo_batches.is_empty() {
                return Err(EngineError::EngineBug {
                    location: "Buffer::undo",
                    detail: format!("历史节点 {node_id:?} 没有 undo 批次"),
                });
            }
            UndoTarget {
                transaction_id: node.entry.transaction_id,
                undo_batches: node.entry.undo_batches.clone(),
            }
        };
        self.history
            .step_undo()
            .ok_or_else(|| EngineError::EngineBug {
                location: "Buffer::undo",
                detail: "已验证的当前历史节点无法执行 undo 步进".to_string(),
            })?;

        let mut result = None;
        for tx_edits in undo_target.undo_batches.iter() {
            let (_, delta, changeset) = self.apply_edit_list(
                self.version,
                tx_edits.clone(), // EditList::clone 是 O(1) Arc 递增
                TransactionSource::Undo,
            )?;
            result = Some((delta, changeset));
        }

        let result = result.ok_or_else(|| EngineError::EngineBug {
            location: "Buffer::undo",
            detail: "已验证的 undo 批次没有产生回放结果".to_string(),
        })?;
        Ok(Some(HistoryEditOutcome::new(
            undo_target.transaction_id,
            result.0,
            result.1,
        )))
    }

    /// 重做沿默认分支（最近创建子节点链）的下一个节点。
    ///
    /// 没有可 redo 节点时返回 `Ok(None)`。
    pub fn redo(&mut self) -> EngineResult<Option<HistoryEditOutcome>> {
        self.ensure_writable()?;
        let Some(target) = self.history.default_redo_target() else {
            return Ok(None);
        };
        self.redo_into_branch(target).map(Some)
    }

    /// 重做到指定的 redo 分支节点。
    ///
    /// `node_id` 必须是 `redo_branches()` 之一，否则返回 `EngineError::InvalidHistoryBranch`。
    pub fn redo_to_branch(&mut self, node_id: HistoryNodeId) -> EngineResult<HistoryEditOutcome> {
        if !self.history.children_of_current().contains(&node_id) {
            return Err(EngineError::InvalidHistoryBranch(node_id));
        }
        self.redo_into_branch(node_id)
    }

    fn redo_into_branch(&mut self, node_id: HistoryNodeId) -> EngineResult<HistoryEditOutcome> {
        self.ensure_writable()?;

        let target = {
            let node = self
                .history
                .node(node_id)
                .ok_or_else(|| EngineError::EngineBug {
                    location: "Buffer::redo_into_branch",
                    detail: format!("redo 目标节点 {node_id:?} 缺失"),
                })?;
            if node.entry.redo_batches.is_empty() {
                return Err(EngineError::EngineBug {
                    location: "Buffer::redo_into_branch",
                    detail: format!("历史节点 {node_id:?} 没有 redo 批次"),
                });
            }
            RedoTarget {
                transaction_id: node.entry.transaction_id,
                redo_batches: node.entry.redo_batches.clone(),
            }
        };
        self.history
            .step_redo_into(node_id)
            .ok_or_else(|| EngineError::EngineBug {
                location: "Buffer::redo_into_branch",
                detail: format!("已验证的 redo 目标节点 {node_id:?} 不是当前历史节点的子节点"),
            })?;

        let mut result = None;
        for tx_edits in target.redo_batches.iter() {
            let (_, delta, changeset) =
                self.apply_edit_list(self.version, tx_edits.clone(), TransactionSource::Redo)?;
            result = Some((delta, changeset));
        }

        let result = result.ok_or_else(|| EngineError::EngineBug {
            location: "Buffer::redo_into_branch",
            detail: "已验证的 redo 批次没有产生回放结果".to_string(),
        })?;
        Ok(HistoryEditOutcome::new(
            target.transaction_id,
            result.0,
            result.1,
        ))
    }

    pub(in crate::buffer) fn push_history(
        &mut self,
        entry: HistoryEntry,
        metadata: &TransactionMetadata,
    ) -> EngineResult<Option<TransactionId>> {
        if metadata.merge_policy() == TransactionMergePolicy::MergeWithPrevious
            && self.history.merge_into_current(entry.clone())
        {
            let transaction_id = self
                .history
                .current()
                .and_then(|id| self.history.node(id))
                .map(|node| node.entry.transaction_id)
                .ok_or_else(|| EngineError::EngineBug {
                    location: "Buffer::push_history",
                    detail: "合并成功后当前历史节点缺失".to_string(),
                })?;
            self.truncate_undo_history_to_budget();
            return Ok(self
                .history
                .current()
                .and_then(|id| self.history.node(id))
                .filter(|node| node.entry.transaction_id == transaction_id)
                .map(|node| node.entry.transaction_id));
        }

        let transaction_id = entry.transaction_id;
        self.history.push_child(entry)?;
        self.truncate_undo_history_to_budget();
        Ok(self
            .history
            .current()
            .and_then(|id| self.history.node(id))
            .filter(|node| node.entry.transaction_id == transaction_id)
            .map(|node| node.entry.transaction_id))
    }

    /// 当 `record_history=false` 提交后清掉当前节点下的所有 redo 分支：
    /// 未记录的文本变化已让这些分支的回放数据失效。
    pub(in crate::buffer) fn drop_unrecorded_redo_branches(&mut self) {
        self.history.drop_children_of_current();
    }

    /// 替换 `LargeFilePolicy` 并立即按新预算截断历史；不影响当前文本或版本。
    ///
    /// 调用时机典型场景：宿主在加载完文件 / 检测到大文件后需要把 Undo 预算调小，
    /// 引擎按新预算从最老的非 current 叶子开始丢弃节点，直到 ≤ 预算或没有可丢叶子。
    pub fn set_large_file_policy(&mut self, policy: LargeFilePolicy) {
        self.config.large_file = policy;
        self.truncate_undo_history_to_budget();
    }

    pub(in crate::buffer) fn truncate_undo_history_to_budget(&mut self) {
        let policy = &self.config.large_file;
        self.history
            .truncate_to_budget(policy.max_undo_history, policy.max_undo_history_bytes);
    }

    /// 构造 `edits` 的逆操作 `EditList`，用于 Undo 回放。
    ///
    /// **复用 `PositionMap`**：旧文本坐标 → 新文本坐标的映射委托给同一份算法，
    /// 不再手搓 `(old_start as isize + diff).max(0) as usize` 的脆弱 diff 算术。
    /// 字节长度直接来自 `Edit::replacement().len()`，O(1)；旧文本切片走
    /// `storage.slice_text`（单块时 `Cow::Borrowed` 零拷贝）。
    pub(in crate::buffer) fn build_inverse_edit_list(
        &self,
        edits: &EditList,
    ) -> EngineResult<EditList> {
        let position_map = crate::position_map::PositionMap::from_edits(edits.as_slice().to_vec());

        let mut inverse = Vec::with_capacity(edits.len());

        for edit in edits.as_slice() {
            // 取出删除掉的旧文本（用作 Undo 时的回填内容）
            let deleted_text = self.slice_text(edit.range())?.to_string();

            // 旧位置 → 新位置（与 ChangeSet::changed_ranges 用同一算法）
            let new_start = position_map
                .map_old_position_with_affinity(
                    edit.range().start(),
                    crate::position_map::Affinity::Before,
                )
                .value();
            let replacement_bytes = edit.replacement().len();
            let new_end =
                new_start
                    .checked_add(replacement_bytes)
                    .ok_or_else(|| EngineError::EngineBug {
                        location: "build_inverse_edit_list",
                        detail: "构造反向区间时字节偏移溢出".to_string(),
                    })?;

            let new_range = crate::TextRange::new(new_start, new_end)?;
            inverse.push(Edit::replace(new_range, deleted_text));
        }

        Ok(EditList::new(inverse)?)
    }
}

struct UndoTarget {
    transaction_id: TransactionId,
    /// `Arc::clone` 是 O(1)；批次本身复用历史节点拥有的存储。
    undo_batches: Arc<[EditList]>,
}

struct RedoTarget {
    transaction_id: TransactionId,
    redo_batches: Arc<[EditList]>,
}
