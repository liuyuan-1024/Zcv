use super::*;
use crate::{errors::TransactionError, transaction::EditList, types::BufferVersion};

#[test]
fn transaction_empty_edit_list_should_be_rejected_before_state_transition() {
    let err =
        Transaction::new(BufferVersion::INITIAL, EditList::new(Vec::new()).unwrap()).unwrap_err();

    assert_eq!(err, TransactionError::EmptyTransaction);
}
