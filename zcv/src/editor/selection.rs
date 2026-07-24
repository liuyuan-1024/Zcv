//! Editor 视图选区历史。

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use zcv_engine::{SelectionSet, TransactionId};

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
