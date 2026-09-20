use super::*;
use crate::{BufferConfig, ByteOffset, Edit};

#[test]
fn stale_transaction_is_rejected_before_text_version_and_history_change() {
    let mut buffer = Buffer::from_text("abc".to_owned(), BufferConfig::default()).unwrap();
    let stale_version = buffer.version();
    buffer
        .edit(
            [Edit::insert(ByteOffset::new(3), "!").unwrap()],
            Default::default(),
        )
        .unwrap();
    let current_version = buffer.version();
    let can_undo = buffer.can_undo();
    let can_redo = buffer.can_redo();
    let transaction = Transaction::from_edits(
        stale_version,
        vec![Edit::insert(ByteOffset::ZERO, "stale").unwrap()],
    )
    .unwrap();

    let error = buffer.apply_transaction(transaction).unwrap_err();

    assert!(matches!(
        error,
        TextError::Transaction(TransactionError::VersionMismatch { expected, actual })
            if expected == current_version && actual == stale_version
    ));
    assert_eq!(buffer.version(), current_version);
    assert_eq!(buffer.can_undo(), can_undo);
    assert_eq!(buffer.can_redo(), can_redo);
    assert_eq!(buffer.len_bytes(), ByteOffset::new(4));
}
