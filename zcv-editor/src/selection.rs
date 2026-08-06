//! Editor 视图选区状态、历史与 selection 编辑语义。
//!
//! Editor 的选区端点以引擎 `Anchor` 表达：
//! 任何文本变更（本编辑器编辑、共享 Buffer 的其他 Editor 编辑、外部加载）之后，统一通过 PositionMap 批量映射端点，选区自动跟随；
//! 消费时按当前 Snapshot 解析为字节偏移。
//! 引擎的 `Selection` / `SelectionSet` 仍是编辑算法与历史快照使用的纯数据原语。

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use crate::display_map::DisplayColumn;
use zcv_engine::{
    Affinity, Anchor, Buffer, BufferVersion, CoordinateError, Edit, EngineResult, PositionMap,
    Selection, SelectionSet, Snapshot, Transaction, TransactionId, TransactionMetadata,
    TransactionOutcome,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EditOutcome {
    transaction: Option<TransactionOutcome>,
}

impl EditOutcome {
    pub(super) fn unchanged() -> Self {
        Self { transaction: None }
    }

    pub(super) fn edited(transaction: TransactionOutcome) -> Self {
        Self {
            transaction: Some(transaction),
        }
    }

    pub(super) fn history_transaction_id(&self) -> Option<TransactionId> {
        self.transaction
            .as_ref()
            .and_then(TransactionOutcome::history_transaction_id)
    }

    pub(super) fn transaction(&self) -> Option<&TransactionOutcome> {
        self.transaction.as_ref()
    }
}

/// 校验并应用一组目标编辑，返回事务结果。
///
/// 目标区间为空且替换文本也为空时不产生编辑；全部无编辑时返回 `None`。
fn apply_edits(
    buffer: &mut Buffer,
    targets: &[(Selection, Arc<str>)],
    metadata: TransactionMetadata,
) -> EngineResult<Option<TransactionOutcome>> {
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
    buffer
        .apply_transaction(
            Transaction::from_edits(buffer.version(), edits)?.with_metadata(metadata),
        )
        .map(Some)
}

pub(super) fn replace_selections(
    buffer: &mut Buffer,
    selections: &SelectionSet,
    replacement: &str,
    metadata: TransactionMetadata,
) -> EngineResult<EditOutcome> {
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

    match apply_edits(buffer, &targets, metadata)? {
        None => Ok(EditOutcome::unchanged()),
        Some(transaction) => Ok(EditOutcome::edited(transaction)),
    }
}

pub(super) fn apply_targeted_edits(
    buffer: &mut Buffer,
    targets: Vec<(Selection, Arc<str>)>,
    metadata: TransactionMetadata,
) -> EngineResult<EditOutcome> {
    match apply_edits(buffer, &targets, metadata)? {
        None => Ok(EditOutcome::unchanged()),
        Some(transaction) => Ok(EditOutcome::edited(transaction)),
    }
}

/// 应用编辑目标，并返回编辑后的选区。
///
/// 行移动等场景的选区需要基于编辑后的行位置重新定位端点， position_map 的默认映射会把删除范围内的点吸附到删除起点，无法跟随整体移动的行块。
pub(super) fn apply_edits_with_after_mapping(
    buffer: &mut Buffer,
    targets: Vec<(Selection, Arc<str>)>,
    metadata: TransactionMetadata,
    map_after: impl FnOnce(&Snapshot) -> EngineResult<SelectionSet>,
) -> EngineResult<(EditOutcome, SelectionSet)> {
    match apply_edits(buffer, &targets, metadata)? {
        None => Ok((EditOutcome::unchanged(), map_after(&buffer.snapshot())?)),
        Some(transaction) => Ok((
            EditOutcome::edited(transaction),
            map_after(&buffer.snapshot())?,
        )),
    }
}

fn validate_selection(snapshot: &Snapshot, selection: Selection) -> EngineResult<()> {
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
    /// 垂直移动持久保留的目标显示列，对齐引擎 `Selection::goal`。
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

    /// 输入语义：替换后光标落在插入文本末尾，所有选区折叠为 head 光标。
    pub(crate) fn collapse_to_heads(&mut self) {
        for selection in &mut self.selections {
            let head = if selection.reversed {
                selection.start
            } else {
                selection.end
            };
            selection.start = head;
            selection.end = head;
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TransactionSelections {
    undo: SelectionSet,
    redo: SelectionSet,
}

impl TransactionSelections {
    pub(super) fn undo(&self) -> &SelectionSet {
        &self.undo
    }

    pub(super) fn redo(&self) -> &SelectionSet {
        &self.redo
    }
}

#[derive(Debug, Default)]
pub(super) struct SelectionHistory {
    selections_by_transaction: HashMap<TransactionId, TransactionSelections>,
}

impl SelectionHistory {
    pub(super) fn record_transaction(
        &mut self,
        transaction_id: TransactionId,
        undo: SelectionSet,
        redo: SelectionSet,
    ) {
        match self.selections_by_transaction.entry(transaction_id) {
            Entry::Occupied(mut entry) => entry.get_mut().redo = redo,
            Entry::Vacant(entry) => {
                entry.insert(TransactionSelections { undo, redo });
            }
        }
    }

    pub(super) fn transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Option<&TransactionSelections> {
        self.selections_by_transaction.get(&transaction_id)
    }
}
