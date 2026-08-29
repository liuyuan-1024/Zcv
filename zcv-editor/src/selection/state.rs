//! Editor 视图选区状态、历史与 selection 编辑语义。
//!
//! Editor 的选区端点以 zcv-text `Anchor` 表达：
//! 任何文本变更（本编辑器编辑、共享 Buffer 的其他 Editor 编辑、外部加载）之后，统一通过 PositionMap 批量映射端点，选区自动跟随；
//! 消费时按当前 Snapshot 解析为字节偏移。
//! `Selection` / `SelectionSet` 是编辑算法与历史快照使用的 Editor 领域原语。

use std::collections::HashMap;
use std::sync::Arc;

use zcv_text::{
    Affinity, Anchor, Buffer, BufferVersion, ByteOffset, CoordinateError, Edit, PositionMap,
    Snapshot, TextResult, TransactionId, TransactionMetadata, TransactionOutcome,
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

/// 单个选区：两端点以 Anchor 表达，编辑后由 PositionMap 映射自动跟随。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EditorSelection {
    /// 选区左端。`Affinity::Before`：左端边界处的插入吸附在插入文本之前，
    /// 选区不被边界处新文本撑大。
    start: Anchor,
    /// 选区右端。`Affinity::After`：右端边界处的插入吸附在插入文本之后。
    end: Anchor,
    /// 方向：anchor 在右端、head 在左端时为 true。
    reversed: bool,
    /// 垂直移动持久保留的目标显示列。
    goal: Option<DisplayColumn>,
}

impl EditorSelection {
    fn from_selection(version: BufferVersion, selection: Selection) -> Self {
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
            start: Anchor::new(version, start).with_affinity(start_affinity),
            end: Anchor::new(version, end).with_affinity(Affinity::After),
            reversed: selection.is_reversed(),
            goal: selection.goal().map(DisplayColumn::new),
        }
    }

    fn to_selection(self) -> Selection {
        let start = self.start.offset();
        let end = self.end.offset();
        let (anchor, head) = if self.reversed {
            (end, start)
        } else {
            (start, end)
        };
        Selection::new(anchor, head).with_goal(self.goal.map(DisplayColumn::get))
    }
}

/// Editor 视图层的选区集合：端点锚点统一绑定一个 BufferVersion。
///
/// 版本不变量：`version` 始终等于端点锚点所属的文本版本。
/// 任何版本推进后，必须先用对应 PositionMap 调用 [`EditorSelections::map_through_position_map`]推进版本，才能在消费端 [`EditorSelections::resolve`] 出有效偏移。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorSelections {
    version: BufferVersion,
    selections: Vec<EditorSelection>,
    primary_index: usize,
}

impl EditorSelections {
    /// 把 offset 版选区集合重锚定到指定版本。
    pub(crate) fn from_selection_set(version: BufferVersion, set: &SelectionSet) -> Self {
        Self {
            version,
            selections: set
                .as_slice()
                .iter()
                .copied()
                .map(|selection| EditorSelection::from_selection(version, selection))
                .collect(),
            primary_index: set.primary_index(),
        }
    }

    pub(crate) fn version(&self) -> BufferVersion {
        self.version
    }

    /// 按当前快照解析为 offset 版选区集合。
    ///
    /// 端点锚点必须与快照同版本；
    /// 版本不一致说明某次版本推进漏掉了映射，属于编程错误，直接 panic。
    pub(crate) fn resolve(&self, snapshot: &Snapshot) -> SelectionSet {
        assert_eq!(
            self.version,
            snapshot.version(),
            "Editor 选区端点锚点版本与快照版本不一致：{:?} != {:?}",
            self.version,
            snapshot.version()
        );
        SelectionSet::new_with_primary(
            self.selections
                .iter()
                .copied()
                .map(EditorSelection::to_selection)
                .collect(),
            self.primary_index,
        )
    }

    /// 用一次文本变更的 PositionMap 批量映射全部端点锚点。
    ///
    /// `old_version` 必须是当前锚点版本；映射成功后版本推进到 `new_version`。
    /// 端点落在被删除内容中时塌缩到删除起点。
    pub(crate) fn map_through_position_map(
        &mut self,
        old_version: BufferVersion,
        new_version: BufferVersion,
        position_map: &PositionMap,
    ) {
        assert_eq!(
            self.version, old_version,
            "Editor 选区端点锚点版本与映射源版本不一致：{:?} != {:?}",
            self.version, old_version
        );
        for selection in &mut self.selections {
            selection.start = selection
                .start
                .map_through_position_map(new_version, position_map)
                .value();
            selection.end = selection
                .end
                .map_through_position_map(new_version, position_map)
                .value();
        }
        self.version = new_version;
    }
}

impl Default for EditorSelections {
    fn default() -> Self {
        Self {
            version: BufferVersion::INITIAL,
            selections: Vec::new(),
            primary_index: 0,
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
