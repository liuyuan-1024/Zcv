//! History public API：把 Undo / Redo + 本地分支能力暴露为 Buffer 方法。
//!
//! 本文件负责历史图的 cursor 移动、分支查询与重放，不定义 HistoryEntry 的存储形态。

use crate::{
    EngineError, EngineResult, LargeFilePolicy, SelectionSet, TransactionSource,
    buffer::Buffer,
    transaction::{ChangeSet, Delta, Edit, EditList, TransactionMergePolicy, TransactionMetadata},
};

use super::{HistoryEntry, HistoryNodeId, HistoryStatus};

/// 当前历史节点的只读视图，用于宿主感知节点身份、分支结构与选区。
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
    /// 进入节点前的选区。
    pub before_selection: SelectionSet,
    /// 离开节点后的选区。
    pub after_selection: SelectionSet,
    /// 节点描述（来自 `TransactionMetadata::description`）。
    pub description: Option<String>,
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
            before_selection: node.entry.before_selection.clone(),
            after_selection: node.entry.after_selection.clone(),
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
    pub fn undo(&mut self) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.ensure_writable()?;
        self.cancel_composition_before_text_edit()?;

        let undo_target = {
            let Some(node) = self.history.step_undo() else {
                return Ok(None);
            };
            UndoTarget {
                undo_batches: node.entry.undo_batches.clone(),
                before_selection: node.entry.before_selection.clone(),
            }
        };

        let mut result = None;
        for tx_edits in &undo_target.undo_batches {
            result = Some(self.apply_edit_list(
                self.version,
                tx_edits.clone(),
                TransactionSource::Undo,
            )?);
        }

        let result = result.expect("history node must contain at least one undo batch");
        self.selection = undo_target.before_selection;

        Ok(Some(result))
    }

    /// 重做沿默认分支（最近创建子节点链）的下一个节点。
    ///
    /// 没有可 redo 节点时返回 `Ok(None)`。
    pub fn redo(&mut self) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.ensure_writable()?;
        let Some(target) = self.history.default_redo_target() else {
            return Ok(None);
        };
        self.redo_into_branch(target).map(Some)
    }

    /// 重做到指定的 redo 分支节点。
    ///
    /// `node_id` 必须是 `redo_branches()` 之一，否则返回 `EngineError::InvalidHistoryBranch`。
    pub fn redo_to_branch(&mut self, node_id: HistoryNodeId) -> EngineResult<(Delta, ChangeSet)> {
        if !self.history.children_of_current().contains(&node_id) {
            return Err(EngineError::InvalidHistoryBranch(node_id));
        }
        self.redo_into_branch(node_id)
    }

    fn redo_into_branch(&mut self, node_id: HistoryNodeId) -> EngineResult<(Delta, ChangeSet)> {
        self.ensure_writable()?;
        self.cancel_composition_before_text_edit()?;

        let target = {
            let node = self
                .history
                .step_redo_into(node_id)
                .expect("redo_into_branch 必须传入 children_of_current() 之一");
            RedoTarget {
                redo_batches: node.entry.redo_batches.clone(),
                after_selection: node.entry.after_selection.clone(),
            }
        };

        let mut result = None;
        for tx_edits in &target.redo_batches {
            result = Some(self.apply_edit_list(
                self.version,
                tx_edits.clone(),
                TransactionSource::Redo,
            )?);
        }

        let result = result.expect("history node must contain at least one redo batch");
        self.selection = target.after_selection;
        Ok(result)
    }

    pub(in crate::buffer) fn push_history(
        &mut self,
        entry: HistoryEntry,
        metadata: &TransactionMetadata,
    ) -> EngineResult<()> {
        if metadata.merge_policy() == TransactionMergePolicy::MergeWithPrevious
            && self.history.merge_into_current(entry.clone())
        {
            self.truncate_undo_history_to_budget();
            return Ok(());
        }

        self.history.push_child(entry);
        self.truncate_undo_history_to_budget();
        Ok(())
    }

    /// 当 `record_history=false` 提交后清掉当前节点下的所有 redo 分支：
    /// 未记录的文本变化已让这些分支的回放数据失效。
    pub(in crate::buffer) fn drop_unrecorded_redo_branches(&mut self) {
        self.history.drop_children_of_current();
    }

    /// 替换 `LargeFilePolicy` 并立即按新预算截断历史；不影响当前文本、版本或 selection。
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

    pub(in crate::buffer) fn build_inverse_edit_list(
        &self,
        edits: &EditList,
    ) -> EngineResult<EditList> {
        let mut inverse = Vec::with_capacity(edits.len());
        let mut diff = 0isize;

        for edit in edits.as_slice() {
            let old_start = edit.range().start().get();
            let old_end = edit.range().end().get();
            let deleted_text = self.slice_text(edit.range())?.to_string();

            let new_start = (old_start as isize + diff).max(0) as usize;
            // ByteOffset 深核：用 byte 长度（无需 chars().count() 的 O(N) 扫描）
            let replacement_bytes = edit.replacement().len();
            let new_end = new_start + replacement_bytes;
            let new_range = crate::TextRange::new(
                crate::ByteOffset::new(new_start),
                crate::ByteOffset::new(new_end),
            )?;

            inverse.push(Edit::replace(new_range, deleted_text));

            diff += replacement_bytes as isize - (old_end - old_start) as isize;
        }

        Ok(EditList::new(inverse)?)
    }
}

struct UndoTarget {
    undo_batches: Vec<EditList>,
    before_selection: SelectionSet,
}

struct RedoTarget {
    redo_batches: Vec<EditList>,
    after_selection: SelectionSet,
}
