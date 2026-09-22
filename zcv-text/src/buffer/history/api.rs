//! History public API：把 Undo / Redo + 本地分支能力暴露为 Buffer 方法。
//!
//! 本文件只管理历史图的 cursor 移动与回放编排；可重放编辑来自 `EditLog` 的版本区间。

use super::{HistoryEntry, HistoryNodeId, HistoryState};
use crate::{
    TextError, TextRange, TextResult, TransactionId, TransactionSource,
    buffer::Buffer,
    config::LargeFilePolicy,
    position_map::{Affinity, PositionMap},
    tracking::EditLog,
    transaction::{Edit, EditList, TransactionMergePolicy, TransactionMetadata},
};

/// 一次 Undo / Redo 文本回放的结果。
///
/// 只携带被回放历史节点的规范事务身份；跨批次的复合文本变化由源 Buffer 的订阅权威给出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEditOutcome {
    transaction_id: TransactionId,
}

impl HistoryEditOutcome {
    fn new(transaction_id: TransactionId) -> Self {
        Self { transaction_id }
    }

    /// 被回放历史节点的规范事务身份。
    pub fn transaction_id(&self) -> TransactionId {
        self.transaction_id
    }
}

impl Buffer {
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// 当前历史节点的事务身份；无历史时为 `None`。
    ///
    /// 组合文档用它把源文档历史与自己的选区历史对齐，不暴露历史图内部节点身份。
    pub fn current_history_transaction_id(&self) -> Option<TransactionId> {
        self.history.current_transaction_id()
    }

    /// 撤销最近一次历史节点。
    ///
    /// 没有可撤销历史时返回 `Ok(None)`，避免把空历史当作错误。
    pub fn undo(&mut self) -> TextResult<Option<HistoryEditOutcome>> {
        self.ensure_writable()?;

        let Some(node_id) = self.history.current() else {
            return Ok(None);
        };
        let target = self.history_target(node_id, ReplayKind::Undo)?;
        // 先取齐整个版本区间的可回放编辑，再移动历史游标：日志已裁剪或缺少逆编辑时
        // 不会留下「游标已移动、文本未回退」的半完成状态。
        let transaction_id = target.transaction_id;
        let batches = self
            .edit_log
            .undo_batches(target.start_version, target.end_version)?;
        self.history
            .step_undo()
            .ok_or_else(|| TextError::InvariantViolation {
                location: "Buffer::undo",
                detail: "已验证的当前历史节点无法执行 undo 步进".to_string(),
            })?;
        self.replay_history_batches(target.kind, batches)?;
        Ok(Some(HistoryEditOutcome::new(transaction_id)))
    }

    /// 重做沿默认分支（最近创建子节点链）的下一个节点。
    ///
    /// 没有可 redo 节点时返回 `Ok(None)`。
    pub fn redo(&mut self) -> TextResult<Option<HistoryEditOutcome>> {
        self.ensure_writable()?;
        let Some(node_id) = self.history.default_redo_target() else {
            return Ok(None);
        };
        let target = self.history_target(node_id, ReplayKind::Redo)?;
        // 与 undo 相同：先取齐编辑日志批次再推进游标。
        let transaction_id = target.transaction_id;
        let batches = self
            .edit_log
            .redo_batches(target.start_version, target.end_version)?;
        self.history
            .step_redo_into(node_id)
            .ok_or_else(|| TextError::InvariantViolation {
                location: "Buffer::redo",
                detail: format!("已验证的 redo 目标节点 {node_id:?} 不是当前历史节点的子节点"),
            })?;
        self.replay_history_batches(target.kind, batches)?;
        Ok(Some(HistoryEditOutcome::new(transaction_id)))
    }

    /// 取历史节点对应的版本区间与事务身份；undo / redo 只差一个回放方向。
    fn history_target(&self, node_id: HistoryNodeId, kind: ReplayKind) -> TextResult<ReplayTarget> {
        let node = self
            .history
            .node(node_id)
            .ok_or_else(|| TextError::InvariantViolation {
                location: "Buffer::history_target",
                detail: format!("回放目标节点 {node_id:?} 缺失"),
            })?;
        let entry = &node.entry;
        if entry.start_version == entry.end_version {
            return Err(TextError::InvariantViolation {
                location: "Buffer::history_target",
                detail: format!("历史节点 {node_id:?} 没有版本推进"),
            });
        }

        Ok(ReplayTarget {
            transaction_id: entry.transaction_id,
            start_version: entry.start_version,
            end_version: entry.end_version,
            kind,
        })
    }

    /// 按已取齐的批次回放；调用方已在移动历史游标前完成日志查询。
    fn replay_history_batches(
        &mut self,
        kind: ReplayKind,
        batches: Vec<EditList>,
    ) -> TextResult<()> {
        for tx_edits in batches {
            self.apply_edit_list(self.version, tx_edits, kind.source())?;
        }
        self.truncate_edit_history_to_budget();
        Ok(())
    }

    /// `MergeWithPrevious` 会把「当前节点终点 → 新事务起点」之间的全部编辑日志条目
    /// 一并并入节点，因此该区间必须逐条保留逆编辑：否则后续 undo 回放到缺失逆编辑的
    /// 条目会报错。回放（undo/redo）产生的条目与普通提交一样可回放，放弃历史的大事务则不可。
    pub(in crate::buffer) fn push_history(
        &mut self,
        entry: HistoryEntry,
        metadata: &TransactionMetadata,
    ) -> TextResult<Option<TransactionId>> {
        push_history_into(&mut self.history, &self.edit_log, entry, metadata)
    }

    /// 当 `record_history=false` 提交后清掉当前节点下的所有 redo 分支：
    /// 未记录的文本变化已让这些分支的回放数据失效。
    pub(in crate::buffer) fn drop_unrecorded_redo_branches(&mut self) {
        self.history.drop_children_of_current();
    }

    /// 按编辑历史预算裁剪日志，并同步丢弃超出保留窗口的历史节点。
    pub(in crate::buffer) fn truncate_edit_history_to_budget(&mut self) {
        truncate_edit_history(
            &mut self.edit_log,
            &mut self.history,
            &self.config.large_file,
        );
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
    ) -> TextResult<EditList> {
        let position_map = PositionMap::from_edits(edits.as_slice());

        let mut inverse = Vec::with_capacity(edits.len());

        for edit in edits.as_slice() {
            // 取出删除掉的旧文本（用作 Undo 时的回填内容）
            let deleted_text = self.slice_text(edit.range())?.to_string();

            // 旧位置 → 新位置（与 ChangeSet::changed_ranges 用同一算法）。
            // 单点映射在事务 edit 数很小时比批量映射更快（零临时分配）。
            let new_start = position_map
                .map_old_position_with_affinity(edit.range().start(), Affinity::Before)
                .value();
            let replacement_bytes = edit.replacement().len();
            let new_end = new_start.checked_add(replacement_bytes).ok_or_else(|| {
                TextError::InvariantViolation {
                    location: "build_inverse_edit_list",
                    detail: "构造反向区间时字节偏移溢出".to_string(),
                }
            })?;

            let new_range = TextRange::new(new_start, new_end)?;
            inverse.push(Edit::replace(new_range, deleted_text));
        }

        Ok(EditList::new(inverse)?)
    }
}

/// 把 `entry` 作为当前节点的新子节点入图；`MergeWithPrevious` 且区间可回放时并入当前节点。
///
/// 供提交管线在克隆的 `HistoryState` 上做计划，调用方负责最终安装。
pub(in crate::buffer) fn push_history_into(
    history: &mut HistoryState,
    edit_log: &EditLog,
    entry: HistoryEntry,
    metadata: &TransactionMetadata,
) -> TextResult<Option<TransactionId>> {
    if metadata.merge_policy() == TransactionMergePolicy::MergeWithPrevious
        && history.current_end_version().is_some_and(|current_end| {
            edit_log.range_is_replayable(current_end, entry.start_version)
        })
        && history.merge_into_current(entry.clone())
    {
        return Ok(history.current_transaction_id());
    }

    history.push_child(entry)?;
    Ok(history.current_transaction_id())
}

/// 按编辑历史预算裁剪日志，并同步丢弃超出保留窗口的历史节点。
pub(in crate::buffer) fn truncate_edit_history(
    edit_log: &mut EditLog,
    history: &mut HistoryState,
    policy: &LargeFilePolicy,
) {
    *edit_log = edit_log.truncated(
        policy.max_edit_history_entries,
        policy.max_edit_history_bytes,
    );
    history.retain_versions_since(edit_log.earliest_version());
    history.truncate_to_node_budget(policy.max_undo_history);
}

/// undo / redo 回放方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplayKind {
    Undo,
    Redo,
}

impl ReplayKind {
    fn source(self) -> TransactionSource {
        match self {
            Self::Undo => TransactionSource::Undo,
            Self::Redo => TransactionSource::Redo,
        }
    }
}

/// 待回放的历史事实：规范事务身份 + 版本区间 + 回放方向。
struct ReplayTarget {
    transaction_id: TransactionId,
    start_version: crate::BufferVersion,
    end_version: crate::BufferVersion,
    kind: ReplayKind,
}
