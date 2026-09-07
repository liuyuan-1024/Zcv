//! Editor 视图选区状态、历史与 selection 编辑语义。
//!
//! Editor 的选区端点以 zcv-text `Anchor` 表达：
//! 任何文本变更（本编辑器编辑、共享 Buffer 的其他 Editor 编辑、外部加载）之后，统一通过 PositionMap 批量映射端点，选区自动跟随；
//! 消费时按当前 Snapshot 解析为字节偏移。
//! `Selection` / `SelectionSet` 是编辑算法与历史快照使用的 Editor 领域原语。

use std::collections::HashMap;
use std::sync::Arc;

use gpui::EntityId;
use zcv_multi_buffer::{MultiBufferAnchor, MultiBufferSnapshot};
use zcv_text::{
    Affinity, Buffer, ByteOffset, CoordinateError, Edit, PositionMap, Snapshot, TextResult,
    TransactionId, TransactionMetadata, TransactionOutcome,
};

use super::{Selection, SelectionSet};
use crate::display_map::DisplayColumn;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditOutcome {
    transaction: Option<TransactionOutcome>,
}

impl EditOutcome {
    pub(crate) fn unchanged() -> Self {
        Self { transaction: None }
    }

    pub(crate) fn edited(transaction: TransactionOutcome) -> Self {
        Self {
            transaction: Some(transaction),
        }
    }

    /// 折叠事务结果：`None`（无实际编辑）视为未变化，`Some` 视为一次编辑。
    pub(crate) fn from_transaction(transaction: Option<TransactionOutcome>) -> Self {
        match transaction {
            Some(transaction) => Self::edited(transaction),
            None => Self::unchanged(),
        }
    }

    pub(crate) fn transaction(&self) -> Option<&TransactionOutcome> {
        self.transaction.as_ref()
    }
}

/// 校验并应用一组目标编辑，返回事务结果。
///
/// 目标区间为空且替换文本也为空时不产生编辑；全部无编辑时返回 `None`。
pub(crate) fn apply_edits(
    buffer: &mut Buffer,
    targets: &[(Selection, Arc<str>)],
    metadata: TransactionMetadata,
) -> TextResult<Option<TransactionOutcome>> {
    let snapshot = buffer.snapshot();
    let mut edits = Vec::with_capacity(targets.len());
    for (selection, replacement) in targets {
        validate_selection(&snapshot, *selection)?;
        let range = selection.range();
        if !(range.is_empty() && replacement.is_empty()) {
            edits.push(Edit::replace(range, Arc::clone(replacement)));
        }
    }
    if edits.is_empty() {
        return Ok(None);
    }
    buffer.edit(edits, metadata).map(Some)
}

pub(crate) fn replace_selections(
    buffer: &mut Buffer,
    selections: &SelectionSet,
    replacement: &str,
    metadata: TransactionMetadata,
) -> TextResult<(EditOutcome, SelectionSet)> {
    let replacement: Arc<str> = Arc::from(replacement);
    let selections = selections.normalized();
    let snapshot = buffer.snapshot();

    // 替换为相同文本或双方均为空时不产生编辑，选区由 Editor 侧锚点映射跟随。
    let mut targets = Vec::with_capacity(selections.len());
    for selection in selections.as_slice() {
        let range = selection.range();
        if !(range.is_empty() && replacement.is_empty())
            && snapshot.slice_text(range)?.as_str() != replacement.as_ref()
        {
            targets.push((*selection, Arc::clone(&replacement)));
        }
    }

    // 替换命令的结果不是让旧选区端点被动跟随 PositionMap，而是显式成为每段插入文本末尾的 caret。
    //
    // 这尤其重要于删除非空选区：无论选区方向、端点 affinity 或同时存在的其他编辑如何，结果都必须是删除起点的单个 caret。
    let (outcome, after_selections) = match apply_edits(buffer, &targets, metadata)? {
        None => (
            EditOutcome::unchanged(),
            SelectionSet::new_with_primary(
                selections
                    .as_slice()
                    .iter()
                    .map(|selection| {
                        Selection::caret(ByteOffset::new(
                            selection.start().get() + replacement.len(),
                        ))
                    })
                    .collect(),
                selections.primary_index(),
            ),
        ),
        Some(transaction) => {
            let position_map = transaction.event().position_map();
            let after_selections = SelectionSet::new_with_primary(
                selections
                    .as_slice()
                    .iter()
                    .map(|selection| {
                        let start = position_map.map_old_position(selection.start()).value();
                        // 插入到空选区时，PositionMap 已将 caret 吸附到插入文本之后；
                        // 非空选区的起点则映射到替换起点，需要跨过替换文本。
                        let end = if selection.is_caret() {
                            start
                        } else {
                            ByteOffset::new(start.get() + replacement.len())
                        };
                        Selection::caret(end)
                    })
                    .collect(),
                selections.primary_index(),
            );
            (EditOutcome::edited(transaction), after_selections)
        }
    };
    Ok((outcome, after_selections))
}

pub(crate) fn apply_targeted_edits(
    buffer: &mut Buffer,
    targets: Vec<(Selection, Arc<str>)>,
    metadata: TransactionMetadata,
) -> TextResult<EditOutcome> {
    match apply_edits(buffer, &targets, metadata)? {
        None => Ok(EditOutcome::unchanged()),
        Some(transaction) => Ok(EditOutcome::edited(transaction)),
    }
}

/// 应用编辑目标，并返回编辑后的选区。
///
/// 行移动等场景的选区需要基于编辑后的行位置重新定位端点， position_map 的默认映射会把删除范围内的点吸附到删除起点，无法跟随整体移动的行块。
pub(crate) fn apply_edits_with_after_mapping(
    buffer: &mut Buffer,
    targets: Vec<(Selection, Arc<str>)>,
    metadata: TransactionMetadata,
    map_after: impl FnOnce(&Snapshot) -> TextResult<SelectionSet>,
) -> TextResult<(EditOutcome, SelectionSet)> {
    match apply_edits(buffer, &targets, metadata)? {
        None => Ok((EditOutcome::unchanged(), map_after(&buffer.snapshot())?)),
        Some(transaction) => Ok((
            EditOutcome::edited(transaction),
            map_after(&buffer.snapshot())?,
        )),
    }
}

fn validate_selection(snapshot: &Snapshot, selection: Selection) -> TextResult<()> {
    for offset in [selection.anchor(), selection.head()] {
        snapshot.slice_byte_range(offset, offset)?;
        if !snapshot.is_grapheme_boundary_byte(offset)? {
            return Err(CoordinateError::InvalidGraphemeBoundary(offset).into());
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
        anchor: &impl Fn(ByteOffset) -> Option<MultiBufferAnchor>,
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
                    .map_old_position_with_affinity(selection.start(), start_affinity)
                    .value();
                let end = position_map
                    .map_old_position_with_affinity(selection.end(), Affinity::After)
                    .value();
                let (anchor, head) = if selection.is_reversed() {
                    (end, start)
                } else {
                    (start, end)
                };
                Selection::new(anchor, head).with_goal(selection.goal())
            })
            .collect(),
        set.primary_index(),
    )
}

/// 把源锚点解析为投影偏移；无锚点（空投影）或源已退出投影时落到投影开头。
fn resolve_anchor_offset(
    snapshot: &MultiBufferSnapshot,
    anchor: &Option<MultiBufferAnchor>,
) -> ByteOffset {
    anchor
        .as_ref()
        .and_then(|anchor| snapshot.resolve_anchor(anchor))
        .unwrap_or(ByteOffset::ZERO)
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
    /// [`EditorSelections::from_selection_set`] 用当前快照锚定；
    /// 编辑落位用重建前映射锚定（[`zcv_multi_buffer::MultiBuffer::anchor_after_edit`]）。
    pub(crate) fn anchored(
        set: &SelectionSet,
        anchor: &impl Fn(ByteOffset) -> Option<MultiBufferAnchor>,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransactionSelections {
    undo: SelectionSet,
    redo: Option<SelectionSet>,
}

impl TransactionSelections {
    pub(crate) fn undo(&self) -> &SelectionSet {
        &self.undo
    }

    pub(crate) fn redo(&self) -> Option<&SelectionSet> {
        self.redo.as_ref()
    }

    /// 事务提交后填入 redo 选区（`end_transaction` 时更新）。
    pub(crate) fn set_redo(&mut self, redo: SelectionSet) {
        self.redo = Some(redo);
    }
}

#[derive(Debug, Default)]
pub(crate) struct SelectionHistory {
    selections_by_transaction: HashMap<TransactionId, TransactionSelections>,
}

impl SelectionHistory {
    /// 事务开始时记录 undo 选区。
    pub(crate) fn insert_transaction(&mut self, transaction_id: TransactionId, undo: SelectionSet) {
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
