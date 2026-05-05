use crate::{
    BufferVersion, SelectionSet,
    transaction::{EditList, TransactionMetadata},
};

pub(in crate::buffer) struct PreparedTransaction {
    pub(in crate::buffer) base_version: BufferVersion,
    pub(in crate::buffer) edits: EditList,
    pub(in crate::buffer) metadata: TransactionMetadata,
    pub(in crate::buffer) before_selection: SelectionSet,
    pub(in crate::buffer) explicit_after_selection: Option<SelectionSet>,
    pub(in crate::buffer) undo_edits: EditList,
    pub(in crate::buffer) redo_edits: EditList,
}
