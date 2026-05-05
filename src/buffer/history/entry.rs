use crate::{
    CharOffset, EngineResult, SelectionSet, TextRange,
    transaction::{Edit, EditList},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::buffer) struct HistoryEntry {
    pub(in crate::buffer) undo_batches: Vec<EditList>,
    pub(in crate::buffer) redo_batches: Vec<EditList>,
    pub(in crate::buffer) before_selection: SelectionSet,
    pub(in crate::buffer) after_selection: SelectionSet,
    pub(in crate::buffer) description: Option<String>,
}

impl HistoryEntry {
    pub(in crate::buffer) fn new(
        undo_edits: EditList,
        redo_edits: EditList,
        before_selection: SelectionSet,
        after_selection: SelectionSet,
        description: Option<String>,
    ) -> Self {
        Self {
            undo_batches: vec![undo_edits],
            redo_batches: vec![redo_edits],
            before_selection,
            after_selection,
            description,
        }
    }

    pub(in crate::buffer) fn from_snapshots(
        before_text: String,
        after_text: String,
        before_selection: SelectionSet,
        after_selection: SelectionSet,
        description: Option<String>,
    ) -> EngineResult<Self> {
        let before_range = TextRange::new(
            CharOffset::ZERO,
            CharOffset::new(before_text.chars().count()),
        )?;
        let after_range = TextRange::new(
            CharOffset::ZERO,
            CharOffset::new(after_text.chars().count()),
        )?;

        let redo_edits = EditList::new(vec![Edit::replace(before_range, after_text.clone())])?;
        let undo_edits = EditList::new(vec![Edit::replace(after_range, before_text.clone())])?;

        Ok(Self::new(
            undo_edits,
            redo_edits,
            before_selection,
            after_selection,
            description,
        ))
    }

    pub(in crate::buffer) fn merge(previous: Self, next: Self) -> Self {
        let mut undo_batches = next.undo_batches;
        undo_batches.extend(previous.undo_batches);

        let mut redo_batches = previous.redo_batches;
        redo_batches.extend(next.redo_batches);

        let description = next.description.or(previous.description);

        Self {
            undo_batches,
            redo_batches,
            before_selection: previous.before_selection,
            after_selection: next.after_selection,
            description,
        }
    }
}
