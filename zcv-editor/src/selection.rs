//! Editor 视图选区状态、历史与 selection 编辑语义。

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use zcv_engine::{
    Buffer, CoordinateError, Edit, EngineResult, PositionMap, Selection, SelectionSet, Snapshot,
    Transaction, TransactionId, TransactionMetadata, TransactionOutcome,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EditOutcome {
    transaction: Option<TransactionOutcome>,
    after_selections: SelectionSet,
}

impl EditOutcome {
    pub(super) fn unchanged(after_selections: SelectionSet) -> Self {
        Self {
            transaction: None,
            after_selections,
        }
    }

    pub(super) fn edited(transaction: TransactionOutcome, after_selections: SelectionSet) -> Self {
        Self {
            transaction: Some(transaction),
            after_selections,
        }
    }

    pub(super) fn history_transaction_id(&self) -> Option<TransactionId> {
        self.transaction
            .as_ref()
            .and_then(TransactionOutcome::history_transaction_id)
    }

    pub(super) fn after_selections(&self) -> &SelectionSet {
        &self.after_selections
    }

    pub(super) fn into_after_selections(self) -> SelectionSet {
        self.after_selections
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

/// 把替换后的选区集合映射为「光标落在替换文本之后」的 caret 集合。
///
/// 空选区（光标）靠 PositionMap 的 `Affinity::After` 天然吸附到插入文本之后，
/// 无需再加长度；非空选区被替换后，光标在选区起点映射值之后追加替换文本长度。
fn after_carets(
    position_map: &PositionMap,
    selections: &SelectionSet,
    replacement_len: usize,
) -> SelectionSet {
    let after = selections
        .as_slice()
        .iter()
        .map(|selection| {
            let start = position_map.map_old_position(selection.start()).value();
            let offset = if selection.range().is_empty() {
                start
            } else {
                start
                    .checked_add(replacement_len)
                    .expect("内部不变量：替换后光标偏移不会溢出")
            };
            Selection::caret(offset)
        })
        .collect();
    SelectionSet::new_with_primary(after, selections.primary_index())
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

    // 替换为相同文本或双方均为空时不产生编辑，光标仍按替换后的落点计算。
    let mut targets = Vec::with_capacity(selections.len());
    for selection in selections.as_slice() {
        let range = selection.range();
        if !(range.is_empty() && replacement.is_empty())
            && snapshot.slice_text(range)?.as_str() != replacement.as_ref()
        {
            targets.push((*selection, Arc::clone(&replacement)));
        }
    }

    let Some(transaction) = apply_edits(buffer, &targets, metadata)? else {
        let after = after_carets(&PositionMap::default(), &selections, replacement.len());
        return Ok(EditOutcome::unchanged(after));
    };
    let after = after_carets(
        &transaction.changeset().position_map(),
        &selections,
        replacement.len(),
    );
    Ok(EditOutcome::edited(transaction, after))
}

pub(super) fn apply_targeted_edits(
    buffer: &mut Buffer,
    targets: Vec<(Selection, Arc<str>)>,
    before: &SelectionSet,
    metadata: TransactionMetadata,
) -> EngineResult<EditOutcome> {
    match apply_edits(buffer, &targets, metadata)? {
        None => Ok(EditOutcome::unchanged(before.clone())),
        Some(transaction) => {
            let after = transaction
                .changeset()
                .position_map()
                .map_selection_set(before);
            Ok(EditOutcome::edited(transaction, after))
        }
    }
}

/// 应用编辑目标，并用编辑后的快照计算编辑后选区。
///
/// 行移动等场景的选区需要基于编辑后的行位置重新定位端点， position_map 的默认映射会把删除范围内的点吸附到删除起点，无法跟随整体移动的行块。
pub(super) fn apply_edits_with_after_mapping(
    buffer: &mut Buffer,
    targets: Vec<(Selection, Arc<str>)>,
    metadata: TransactionMetadata,
    map_after: impl FnOnce(&Snapshot) -> EngineResult<SelectionSet>,
) -> EngineResult<EditOutcome> {
    match apply_edits(buffer, &targets, metadata)? {
        None => Ok(EditOutcome::unchanged(map_after(&buffer.snapshot())?)),
        Some(transaction) => Ok(EditOutcome::edited(
            transaction,
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
