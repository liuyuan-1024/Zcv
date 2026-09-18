//! Editor 视图选区状态、历史与 selection 编辑语义。
//!
//! Editor 的选区端点以 zcv-text `Anchor` 表达：
//! 任何文本变更（本编辑器编辑、共享 Buffer 的其他 Editor 编辑、外部加载）之后，统一通过 PositionMap 批量映射端点，选区自动跟随；
//! 消费时按当前 Snapshot 解析为字节偏移。
//! `Selection` / `SelectionSet` 是编辑算法与历史快照使用的 Editor 领域原语。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::collections::HashMap;
use std::sync::Arc;

use gpui::EntityId;
use zcv_multi_buffer::{MultiBufferAnchor, MultiBufferSnapshot};
use zcv_project::{RegexSearchResult, SearchResult, regex_replacement_for_match};
use zcv_text::{
    Affinity, CoordinateError, Edit, PositionMap, TextError, TextRead, TextResult, TransactionId,
};

use super::{Selection, SelectionSet};
use crate::display_map::DisplayColumn;

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
/// 计划只保存本次操作的 `Edit` 与由其派生的 `PositionMap`；
/// 它直接以 `MultiBufferSnapshot` 校验坐标，绝不复制组合文本或创建临时 `Buffer`。
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
        // `PositionMap` 与底层事务都以编辑前坐标的升序解释同一批编辑。
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
/// 目标区间为空且替换文本也为空时不产生编辑；全部无编辑时返回 `None`。
pub(crate) fn apply_edits(
    plan: &mut EditPlan<'_>,
    targets: &[(Selection, Arc<str>)],
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
    selections: &SelectionSet,
    replacement: &str,
) -> TextResult<(EditOutcome, SelectionSet)> {
    let replacement: Arc<str> = Arc::from(replacement);
    let selections = selections.normalized();
    let snapshot = plan.snapshot();

    // 替换为相同文本或双方均为空时不产生编辑，选区由 Editor 侧锚点映射跟随。
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
    targets: Vec<(Selection, Arc<str>)>,
) -> TextResult<EditOutcome> {
    apply_edits(plan, &targets)
}

fn validate_selection(snapshot: &MultiBufferSnapshot, selection: Selection) -> TextResult<()> {
    for offset in [selection.anchor(), selection.head()] {
        snapshot
            .text_for_range(MultiBufferRange::new(offset, offset).expect("零宽选区必须合法"))?;
        if !snapshot.is_grapheme_boundary(offset.into())? {
            return Err(CoordinateError::InvalidGraphemeBoundary(offset.into()).into());
        }
    }
    Ok(())
}

/// 单个选区：两端点以源锚点表达（绑定底层源文件坐标，而非投影坐标）。
///
/// 源锚点选区是单一数据源：投影重建（reclip/折叠/undo）不改变源，选区无需重映射，消费时按当前 [`MultiBufferSnapshot`] 解析为投影偏移；
/// 源自身变更时经源 PositionMap 推进（保留 affinity）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorSelection {
    /// 选区左端源锚点；`None` 表示空投影（无源可锚定），解析落到投影开头。
    start: Option<MultiBufferAnchor>,
    /// 选区右端源锚点。
    end: Option<MultiBufferAnchor>,
    /// 左端经源变更推进时的吸附方向：`Before` 使边界插入不撑大选区左端（caret 两端均 `After`）。
    start_affinity: Affinity,
    /// 右端经源变更推进时的吸附方向：`After`。
    end_affinity: Affinity,
    /// 方向：anchor 在右端、head 在左端时为 true。
    reversed: bool,
    /// 垂直移动持久保留的目标显示列。
    goal: Option<DisplayColumn>,
}

impl EditorSelection {
    fn from_selection(
        selection: Selection,
        anchor: &impl Fn(MultiBufferOffset) -> Option<MultiBufferAnchor>,
    ) -> Self {
        let start = selection.start();
        let end = selection.end();
        // 光标（零宽）两端都吸附在插入文本之后；
        // 非空选区左端吸附在插入前、右端吸附在插入后，边界处插入不撑大选区左端。
        let start_affinity = if selection.is_caret() {
            Affinity::After
        } else {
            Affinity::Before
        };
        Self {
            start: anchor(start),
            end: anchor(end),
            start_affinity,
            end_affinity: Affinity::After,
            reversed: selection.is_reversed(),
            goal: selection.goal().map(DisplayColumn::new),
        }
    }

    fn to_selection(&self, snapshot: &MultiBufferSnapshot) -> Selection {
        let start = resolve_anchor_offset(snapshot, &self.start);
        let end = resolve_anchor_offset(snapshot, &self.end);
        let (anchor, head) = if self.reversed {
            (end, start)
        } else {
            (start, end)
        };
        Selection::new(anchor, head).with_goal(self.goal.map(DisplayColumn::get))
    }
}

/// 把投影 offset 选区经 PositionMap 推进（caret 两端 `After`；非空选区左端 `Before`、右端 `After`）。
///
/// 通用编辑路径（未显式重算编辑后选区）用事务坐标映射让选区跟随文本变化，得到「编辑后、重建前」投影坐标。
pub(crate) fn map_selection_set(set: &SelectionSet, position_map: &PositionMap) -> SelectionSet {
    SelectionSet::new_with_primary(
        set.as_slice()
            .iter()
            .map(|selection| {
                let start_affinity = if selection.is_caret() {
                    Affinity::After
                } else {
                    Affinity::Before
                };
                let start = position_map
                    .map_old_position_with_affinity(selection.start().into(), start_affinity)
                    .value();
                let end = position_map
                    .map_old_position_with_affinity(selection.end().into(), Affinity::After)
                    .value();
                let (anchor, head) = if selection.is_reversed() {
                    (end, start)
                } else {
                    (start, end)
                };
                Selection::new(anchor.into(), head.into()).with_goal(selection.goal())
            })
            .collect(),
        set.primary_index(),
    )
}

/// 把源锚点解析为投影偏移；无锚点（空投影）或源已退出投影时落到投影开头。
fn resolve_anchor_offset(
    snapshot: &MultiBufferSnapshot,
    anchor: &Option<MultiBufferAnchor>,
) -> MultiBufferOffset {
    anchor
        .as_ref()
        .and_then(|anchor| snapshot.resolve_anchor(anchor))
        .unwrap_or(MultiBufferOffset::ZERO)
}

/// Editor 视图层的选区集合：端点以源锚点表达（单一数据源，不绑定投影版本）。
///
/// 投影重建不改变源，选区无需重映射：消费时按当前 [`MultiBufferSnapshot`] 解析为投影偏移即天然跟随重建。
/// 源自身变更（外部编辑、共享 Buffer 的其他 Editor 编辑）经 [`EditorSelections::map_through_source_change`] 推进源锚点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorSelections {
    selections: Vec<EditorSelection>,
    primary_index: usize,
}

impl EditorSelections {
    /// 把投影 offset 版选区集合按快照锚定为源锚点选区。
    ///
    /// `snapshot` 必须是 `set` 中偏移所属的投影快照；空投影下偏移无法锚定，端点存 `None`（解析回投影开头）。
    pub(crate) fn from_selection_set(snapshot: &MultiBufferSnapshot, set: &SelectionSet) -> Self {
        Self::anchored(set, &|offset| snapshot.anchor_for_offset(offset))
    }

    /// 用自定义锚定把投影 offset 选区转为源锚点选区。
    ///
    /// [`EditorSelections::from_selection_set`] 与编辑落位都用当前快照锚定；
    /// 投影重建不改变源，编辑后偏移直接按当前快照解析为源 Anchor。
    pub(crate) fn anchored(
        set: &SelectionSet,
        anchor: &impl Fn(MultiBufferOffset) -> Option<MultiBufferAnchor>,
    ) -> Self {
        Self {
            selections: set
                .as_slice()
                .iter()
                .map(|selection| EditorSelection::from_selection(*selection, anchor))
                .collect(),
            primary_index: set.primary_index(),
        }
    }

    /// 按快照把源锚点解析为投影 offset 版选区集合。
    pub(crate) fn resolve(&self, snapshot: &MultiBufferSnapshot) -> SelectionSet {
        SelectionSet::new_with_primary(
            self.selections
                .iter()
                .map(|selection| selection.to_selection(snapshot))
                .collect(),
            self.primary_index,
        )
    }

    /// 源自身变更后，把绑定该源的端点源锚点经源 PositionMap 推进（保留 affinity）。
    ///
    /// 投影重建不调用本方法：重建不改变源，源锚点直接按重建后快照解析即跟随。
    pub(crate) fn map_through_source_change(
        &mut self,
        source_id: EntityId,
        position_map: &PositionMap,
    ) {
        for selection in &mut self.selections {
            let start_affinity = selection.start_affinity;
            let end_affinity = selection.end_affinity;
            if let Some(start) = &mut selection.start {
                start.map_through_source_change(source_id, position_map, start_affinity);
            }
            if let Some(end) = &mut selection.end {
                end.map_through_source_change(source_id, position_map, end_affinity);
            }
        }
    }
}

/// 一个事务的选区快照；`redo` 在事务提交时才填入。
///
/// 存源锚点而非投影坐标：撤销/重做后 diff 投影可能异步重建，源锚点不依赖重建时机即可解析。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransactionSelections {
    undo: EditorSelections,
    redo: Option<EditorSelections>,
}

impl TransactionSelections {
    pub(crate) fn undo(&self) -> &EditorSelections {
        &self.undo
    }

    pub(crate) fn redo(&self) -> Option<&EditorSelections> {
        self.redo.as_ref()
    }

    /// 事务提交后填入 redo 选区（`end_transaction` 时更新）。
    pub(crate) fn set_redo(&mut self, redo: EditorSelections) {
        self.redo = Some(redo);
    }
}

#[derive(Debug, Default)]
pub(crate) struct SelectionHistory {
    selections_by_transaction: HashMap<TransactionId, TransactionSelections>,
}

impl SelectionHistory {
    /// 事务开始时记录 undo 选区（源锚点）。
    pub(crate) fn insert_transaction(
        &mut self,
        transaction_id: TransactionId,
        undo: EditorSelections,
    ) {
        self.selections_by_transaction
            .entry(transaction_id)
            .or_insert_with(|| TransactionSelections { undo, redo: None });
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
