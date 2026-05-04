use crate::{ChangeSet, EngineResult, SelectionSet};

use super::Buffer;

impl Buffer {
    pub fn selection(&self) -> &SelectionSet {
        &self.selection
    }

    pub fn set_selection(&mut self, selection: SelectionSet) -> EngineResult<()> {
        self.validate_selection_set(&selection)?;
        self.selection = selection;
        Ok(())
    }

    pub fn selection_after_edit(
        &self,
        selection: &SelectionSet,
        changeset: &ChangeSet,
    ) -> SelectionSet {
        selection.map_through_changeset(changeset)
    }
}
