//! Editor 视图选区状态、历史与 selection 编辑语义。
//!
//! Editor 的选区端点以 MultiBufferAnchor 表达：
//! 任何文本变更（本编辑器编辑、共享 Buffer 的其他 Editor 编辑、外部加载）之后，
//! 锚点按当前 MultiBufferSnapshot 解析，选区自动跟随；
//! Selection/SelectionSet 是编辑算法与历史快照共用的 Editor 领域原语。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::collections::BTreeMap;
use std::sync::Arc;

use zcv_multi_buffer::{MultiBufferAnchor, MultiBufferSnapshot};
use zcv_project::{RegexSearchResult, SearchResult, regex_replacement_for_match};
use zcv_text::{
    CoordinateError, Edit, PositionMap, TextError, TextRead, TextResult, TransactionId,
};

use super::{Selection, SelectionSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditOutcome {
    position_map: Option<PositionMap>,
}

impl EditOutcome {
    pub(crate) fn unchanged() -> Self {
        Self { position_map: None }
    }

    pub(crate) fn edited(position_map: PositionMap) -> Self {
        Self {
            position_map: Some(position_map),
        }
    }

    pub(crate) fn position_map(&self) -> Option<&PositionMap> {
        self.position_map.as_ref()
    }
}

/// 一次 Editor 编辑的投影坐标批次。
///
/// 计划只保存本次操作的 Edit 与由其派生的 PositionMap；
/// 它直接以 MultiBufferSnapshot 校验坐标，绝不复制组合文本或创建临时 Buffer。
pub(crate) struct EditPlan<'a> {
    snapshot: &'a MultiBufferSnapshot,
    edits: Vec<Edit>,
}

impl<'a> EditPlan<'a> {
    pub(crate) fn new(snapshot: &'a MultiBufferSnapshot) -> Self {
        Self {
            snapshot,
            edits: Vec::new(),
        }
    }

    pub(crate) fn snapshot(&self) -> &MultiBufferSnapshot {
        self.snapshot
    }

    pub(crate) fn edit(&mut self, edits: Vec<Edit>) -> TextResult<EditOutcome> {
        let mut changed = Vec::with_capacity(edits.len());
        for edit in edits {
            let range = edit.range();
            let current = self.snapshot.text_for_range(range.into())?;
            if current != edit.replacement() {
                changed.push(edit);
            }
        }
        if changed.is_empty() {
            return Ok(EditOutcome::unchanged());
        }
        // PositionMap 与底层事务都以编辑前坐标的升序解释同一批编辑。
        changed.sort_unstable_by_key(|edit| (edit.range().start(), edit.range().end()));
        let position_map = PositionMap::from_edits(&changed);
        self.edits.extend(changed);
        Ok(EditOutcome::edited(position_map))
    }

    pub(crate) fn into_edits(self) -> Vec<Edit> {
        self.edits
    }

    pub(crate) fn replace_search_match(
        &mut self,
        result: &SearchResult,
        ordinal: usize,
        replacement: &str,
    ) -> TextResult<EditOutcome> {
        self.require_search_version(result.version(), "replace_search_match")?;
        let matched = result
            .match_at(ordinal)
            .ok_or_else(|| TextError::InvariantViolation {
                location: "EditPlan::replace_search_match",
                detail: "搜索匹配不存在".into(),
            })?;
        self.edit(vec![Edit::replace(matched.range(), replacement)])
    }

    pub(crate) fn replace_all_search_matches(
        &mut self,
        result: &SearchResult,
        replacement: &str,
    ) -> TextResult<EditOutcome> {
        self.require_search_version(result.version(), "replace_all_search_matches")?;
        self.edit(
            result
                .ranges()
                .map(|range| Edit::replace(range, replacement))
                .collect(),
        )
    }

    pub(crate) fn replace_regex_match(
        &mut self,
        result: &RegexSearchResult,
        ordinal: usize,
        replacement: &str,
    ) -> TextResult<EditOutcome> {
        self.require_search_version(result.version(), "replace_regex_match")?;
        let (range, replacement) =
            regex_replacement_for_match(self.snapshot, result, ordinal, replacement)
                .map_err(|error| TextError::InvariantViolation {
                    location: "EditPlan::replace_regex_match",
                    detail: format!("正则替换生成失败：{error}"),
                })?
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "EditPlan::replace_regex_match",
                    detail: "搜索匹配不存在".into(),
                })?;
        self.edit(vec![Edit::replace(range, replacement)])
    }

    pub(crate) fn replace_all_regex_matches(
        &mut self,
        result: &RegexSearchResult,
        replacement: &str,
    ) -> TextResult<EditOutcome> {
        self.require_search_version(result.version(), "replace_all_regex_matches")?;
        let edits = zcv_project::regex_replacements_in_text(self.snapshot, result, replacement)
            .map_err(|error| TextError::InvariantViolation {
                location: "EditPlan::replace_all_regex_matches",
                detail: format!("正则替换生成失败：{error}"),
            })?
            .map(|edit| {
                edit.map(|(range, replacement)| Edit::replace(range, replacement))
                    .map_err(|error| TextError::InvariantViolation {
                        location: "EditPlan::replace_all_regex_matches",
                        detail: format!("正则替换生成失败：{error}"),
                    })
            })
            .collect::<TextResult<Vec<_>>>()?;
        self.edit(edits)
    }

    fn require_search_version(
        &self,
        version: zcv_text::BufferVersion,
        operation: &'static str,
    ) -> TextResult<()> {
        if version == self.snapshot.version() {
            return Ok(());
        }
        Err(TextError::InvariantViolation {
            location: operation,
            detail: "搜索结果版本与组合快照不一致".into(),
        })
    }
}

/// 校验并应用一组目标编辑，返回事务结果。
///
/// 目标区间为空且替换文本也为空时不产生编辑；全部无编辑时返回 None。
pub(crate) fn apply_edits(
    plan: &mut EditPlan<'_>,
    targets: &[(Selection<MultiBufferOffset>, Arc<str>)],
) -> TextResult<EditOutcome> {
    let snapshot = plan.snapshot();
    let mut edits = Vec::with_capacity(targets.len());
    for (selection, replacement) in targets {
        validate_selection(snapshot, *selection)?;
        let range = selection.range();
        if !(range.is_empty() && replacement.is_empty()) {
            edits.push(Edit::replace(range.into(), Arc::clone(replacement)));
        }
    }
    if edits.is_empty() {
        return Ok(EditOutcome::unchanged());
    }
    plan.edit(edits)
}

pub(crate) fn replace_selections(
    plan: &mut EditPlan<'_>,
    selections: &SelectionSet<MultiBufferOffset>,
    replacement: &str,
) -> TextResult<(EditOutcome, SelectionSet<MultiBufferOffset>)> {
    let replacement: Arc<str> = Arc::from(replacement);
    let selections = selections.normalized();
    let snapshot = plan.snapshot();

    // 替换为相同文本或双方均为空时不产生编辑，选区由 Editor 侧锚点解析跟随。
    let mut targets = Vec::with_capacity(selections.len());
    for selection in selections.as_slice() {
        let range = selection.range();
        if !(range.is_empty() && replacement.is_empty())
            && snapshot.text_for_range(range)? != replacement.as_ref()
        {
            targets.push((*selection, Arc::clone(&replacement)));
        }
    }

    // 替换命令的结果不是让旧选区端点被动跟随 PositionMap，而是显式成为每段插入文本末尾的 caret。
    //
    // 这尤其重要于删除非空选区：无论选区方向、端点 affinity 或同时存在的其他编辑如何，结果都必须是删除起点的单个 caret。
    let outcome = apply_edits(plan, &targets)?;
    let after_selections = match outcome.position_map() {
        None => SelectionSet::new_with_primary(
            selections
                .as_slice()
                .iter()
                .map(|selection| {
                    Selection::caret(MultiBufferOffset::new(
                        selection.start().get() + replacement.len(),
                    ))
                })
                .collect(),
            selections.primary_index(),
        ),
        Some(position_map) => SelectionSet::new_with_primary(
            selections
                .as_slice()
                .iter()
                .map(|selection| {
                    let start = position_map
                        .map_old_position(selection.start().into())
                        .value();
                    let end = if selection.is_caret() {
                        start.into()
                    } else {
                        MultiBufferOffset::new(start.get() + replacement.len())
                    };
                    Selection::caret(end)
                })
                .collect(),
            selections.primary_index(),
        ),
    };
    Ok((outcome, after_selections))
}

pub(crate) fn apply_targeted_edits(
    plan: &mut EditPlan<'_>,
    targets: Vec<(Selection<MultiBufferOffset>, Arc<str>)>,
) -> TextResult<EditOutcome> {
    apply_edits(plan, &targets)
}

fn validate_selection(
    snapshot: &MultiBufferSnapshot,
    selection: Selection<MultiBufferOffset>,
) -> TextResult<()> {
    for offset in [selection.start(), selection.end()] {
        snapshot
            .text_for_range(MultiBufferRange::new(offset, offset).expect("零宽选区必须合法"))?;
        if !snapshot.is_grapheme_boundary(offset.into())? {
            return Err(CoordinateError::InvalidGraphemeBoundary(offset.into()).into());
        }
    }
    Ok(())
}

/// 一个事务的选区快照；redo 在事务提交时才填入。
///
/// 存源锚点而非投影坐标：撤销/重做后 diff 投影可能异步重建，源锚点不依赖重建时机即可解析。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransactionSelections {
    undo: SelectionSet<MultiBufferAnchor>,
    redo: Option<SelectionSet<MultiBufferAnchor>>,
}

impl TransactionSelections {
    pub(crate) fn undo(&self) -> &SelectionSet<MultiBufferAnchor> {
        &self.undo
    }

    pub(crate) fn redo(&self) -> Option<&SelectionSet<MultiBufferAnchor>> {
        self.redo.as_ref()
    }

    /// 事务提交后填入 redo 选区（end_transaction 时更新）。
    pub(crate) fn set_redo(&mut self, redo: SelectionSet<MultiBufferAnchor>) {
        self.redo = Some(redo);
    }
}

/// 选择历史记录只用于撤销 / 重做时恢复选区，是文本历史的派生缓存。
///
/// 上限与文本层 Undo 历史预算同量级：超出时从最老事务开始丢弃。
/// 被丢弃的事务已不可能再被文本历史撤销 / 重做，因此不影响仍可回放的选区恢复。
const MAX_SELECTION_HISTORY_ENTRIES: usize = 1024;

#[derive(Debug, Default)]
pub(crate) struct SelectionHistory {
    selections_by_transaction: BTreeMap<TransactionId, TransactionSelections>,
}

impl SelectionHistory {
    /// 事务开始时记录 undo 选区（源锚点）；超出上限时丢弃最老事务。
    pub(crate) fn insert_transaction(
        &mut self,
        transaction_id: TransactionId,
        undo: SelectionSet<MultiBufferAnchor>,
    ) {
        self.selections_by_transaction
            .entry(transaction_id)
            .or_insert_with(|| TransactionSelections { undo, redo: None });
        while self.selections_by_transaction.len() > MAX_SELECTION_HISTORY_ENTRIES {
            self.selections_by_transaction.pop_first();
        }
    }

    /// 取事务的选区记录，供提交时更新 redo 选区。
    pub(crate) fn transaction_mut(
        &mut self,
        transaction_id: TransactionId,
    ) -> Option<&mut TransactionSelections> {
        self.selections_by_transaction.get_mut(&transaction_id)
    }

    /// 删除会话合并后留下的孤儿记录（会话并入前节点时自身不再对应历史节点）。
    pub(crate) fn remove_transaction(&mut self, transaction_id: TransactionId) {
        self.selections_by_transaction.remove(&transaction_id);
    }

    pub(crate) fn transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Option<&TransactionSelections> {
        self.selections_by_transaction.get(&transaction_id)
    }
}

#[cfg(test)]
#[path = "test/state_tests.rs"]
mod tests;
