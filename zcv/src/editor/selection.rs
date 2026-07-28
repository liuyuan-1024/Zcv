//! Editor 视图选区状态、历史与 selection 编辑语义。

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use zcv_engine::{
    Buffer, ByteOffset, CoordinateError, Edit, EngineError, EngineResult, Selection, SelectionSet,
    Snapshot, Transaction, TransactionId, TransactionMetadata, TransactionOutcome,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EditOutcome {
    transaction: Option<TransactionOutcome>,
    after_selections: SelectionSet,
}

impl EditOutcome {
    fn unchanged(after_selections: SelectionSet) -> Self {
        Self {
            transaction: None,
            after_selections,
        }
    }

    fn edited(transaction: TransactionOutcome, after_selections: SelectionSet) -> Self {
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

pub(super) fn replace_selections(
    buffer: &mut Buffer,
    selections: &SelectionSet,
    replacement: &str,
    metadata: TransactionMetadata,
) -> EngineResult<EditOutcome> {
    let selections = selections.normalized();
    let snapshot = buffer.snapshot();
    let replacement_len = replacement.len();
    let replacement: Arc<str> = Arc::from(replacement);
    let mut edits = Vec::new();
    let mut after = Vec::with_capacity(selections.len());
    let mut shift = 0isize;

    for selection in selections.as_slice() {
        validate_selection(&snapshot, *selection)?;
        let range = selection.range();
        let new_start = range
            .start()
            .get()
            .checked_add_signed(shift)
            .ok_or_else(offset_arithmetic_bug)?;
        let new_head = new_start
            .checked_add(replacement_len)
            .ok_or_else(offset_arithmetic_bug)?;
        let old_text = snapshot.slice_text(range)?;
        let changed = !(range.is_empty() && replacement.is_empty())
            && old_text.as_str() != replacement.as_ref();
        if changed {
            edits.push(Edit::replace(range, Arc::clone(&replacement)));
            let replacement_len =
                isize::try_from(replacement_len).map_err(|_| offset_arithmetic_bug())?;
            let range_len = isize::try_from(range.len()).map_err(|_| offset_arithmetic_bug())?;
            shift = shift
                .checked_add(replacement_len - range_len)
                .ok_or_else(offset_arithmetic_bug)?;
        }
        after.push(Selection::caret(ByteOffset::new(new_head)));
    }

    let after = SelectionSet::new(after);
    if edits.is_empty() {
        return Ok(EditOutcome::unchanged(after));
    }
    let transaction = buffer.apply_transaction(
        Transaction::from_edits(buffer.version(), edits)?.with_metadata(metadata),
    )?;
    Ok(EditOutcome::edited(transaction, after))
}

pub(super) fn apply_targeted_edits(
    buffer: &mut Buffer,
    targets: Vec<(Selection, Arc<str>)>,
    before: &SelectionSet,
    metadata: TransactionMetadata,
) -> EngineResult<EditOutcome> {
    let snapshot = buffer.snapshot();
    let mut edits = Vec::with_capacity(targets.len());
    for (selection, replacement) in targets {
        validate_selection(&snapshot, selection)?;
        let range = selection.range();
        if !(range.is_empty() && replacement.is_empty()) {
            edits.push(Edit::replace(range, replacement));
        }
    }
    if edits.is_empty() {
        return Ok(EditOutcome::unchanged(before.clone()));
    }
    let transaction = buffer.apply_transaction(
        Transaction::from_edits(buffer.version(), edits)?.with_metadata(metadata),
    )?;
    let after = transaction
        .changeset()
        .position_map()
        .map_selection_set(before);
    Ok(EditOutcome::edited(transaction, after))
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

fn offset_arithmetic_bug() -> EngineError {
    EngineError::EngineBug {
        location: "Editor::replace_selections",
        detail: "映射 selection 编辑时字节偏移溢出".to_string(),
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
