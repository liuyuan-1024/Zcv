use std::thread;

use zom_engine::*;

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap()
}

#[test]
fn single_step_undo_and_redo_restore_text() {
    let mut buffer = buffer("hello");

    buffer.insert(ByteOffset::new(5), " world").unwrap();

    assert_eq!(buffer.text(), "hello world");
    assert_eq!(buffer.history_status().undo_depth, 1);
    assert!(buffer.can_undo());
    assert!(!buffer.can_redo());

    let (undo_delta, _) = buffer.undo().unwrap().expect("expected undo");
    assert_eq!(buffer.text(), "hello");
    assert_eq!(undo_delta.old_version, BufferVersion::new(1));
    assert_eq!(undo_delta.new_version, BufferVersion::new(2));
    assert!(!buffer.can_undo());
    assert!(buffer.can_redo());

    let (redo_delta, _) = buffer.redo().unwrap().expect("expected redo");
    assert_eq!(buffer.text(), "hello world");
    assert_eq!(redo_delta.old_version, BufferVersion::new(2));
    assert_eq!(redo_delta.new_version, BufferVersion::new(3));
    assert!(buffer.can_undo());
    assert!(!buffer.can_redo());
}

#[test]
fn multiple_undo_and_redo_steps_are_lifo() {
    let mut buffer = buffer("");

    buffer.insert(ByteOffset::new(0), "a").unwrap();
    buffer.insert(ByteOffset::new(1), "b").unwrap();
    buffer.insert(ByteOffset::new(2), "c").unwrap();

    assert_eq!(buffer.text(), "abc");
    assert_eq!(buffer.history_status().undo_depth, 3);

    buffer.undo().unwrap();
    assert_eq!(buffer.text(), "ab");

    buffer.undo().unwrap();
    assert_eq!(buffer.text(), "a");

    buffer.redo().unwrap();
    assert_eq!(buffer.text(), "ab");

    buffer.redo().unwrap();
    assert_eq!(buffer.text(), "abc");
}

#[test]
fn new_edit_after_undo_clears_redo_stack() {
    let mut buffer = buffer("abc");

    buffer.insert(ByteOffset::new(3), "d").unwrap();
    buffer.undo().unwrap();
    assert_eq!(buffer.text(), "abc");
    assert!(buffer.can_redo());

    buffer.insert(ByteOffset::new(3), "!").unwrap();

    assert_eq!(buffer.text(), "abc!");
    assert!(!buffer.can_redo());
    assert_eq!(buffer.history_status().undo_depth, 1);
}

#[test]
fn undo_redo_restore_batch_transaction() {
    let mut buffer = buffer("abcdef");
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![
            Edit::replace(range(1, 3), "XX".to_string()),
            Edit::replace(range(4, 6), "Y".to_string()),
        ],
    )
    .unwrap();

    buffer.apply_transaction(tx).unwrap();
    assert_eq!(buffer.text(), "aXXdY");

    buffer.undo().unwrap();
    assert_eq!(buffer.text(), "abcdef");

    buffer.redo().unwrap();
    assert_eq!(buffer.text(), "aXXdY");
}

#[test]
fn undo_redo_restore_selection_snapshots() {
    let mut buffer = buffer("hello");
    let before = SelectionSnapshot::caret(ByteOffset::new(1)).unwrap();
    let after = SelectionSnapshot::caret(ByteOffset::new(4)).unwrap();

    buffer.set_selection_snapshot(Some(before.clone()));

    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(ByteOffset::new(5), "!".to_string()).unwrap()],
    )
    .unwrap()
    .with_selection(Some(before.clone()), Some(after.clone()));

    buffer.apply_transaction(tx).unwrap();
    assert_eq!(buffer.selection_snapshot(), Some(&after));

    buffer.undo().unwrap();
    assert_eq!(buffer.text(), "hello");
    assert_eq!(buffer.selection_snapshot(), Some(&before));

    buffer.redo().unwrap();
    assert_eq!(buffer.text(), "hello!");
    assert_eq!(buffer.selection_snapshot(), Some(&after));
}

#[test]
fn merge_with_previous_creates_single_undo_boundary() {
    let mut buffer = buffer("");

    let first = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(ByteOffset::new(0), "a".to_string()).unwrap()],
    )
    .unwrap();
    buffer.apply_transaction(first).unwrap();

    let second = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(ByteOffset::new(1), "b".to_string()).unwrap()],
    )
    .unwrap()
    .with_metadata(
        TransactionMetadata::new(TransactionSource::Keyboard)
            .with_merge_policy(TransactionMergePolicy::MergeWithPrevious),
    );
    buffer.apply_transaction(second).unwrap();

    assert_eq!(buffer.text(), "ab");
    assert_eq!(buffer.history_status().undo_depth, 1);

    buffer.undo().unwrap();
    assert_eq!(buffer.text(), "");
    assert!(!buffer.can_undo());

    buffer.redo().unwrap();
    assert_eq!(buffer.text(), "ab");
}

#[test]
fn snapshot_is_immutable_versioned_and_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Snapshot>();

    let mut buffer = buffer("line1\nline2");
    let snapshot = buffer.snapshot();

    assert_eq!(snapshot.text(), "line1\nline2");
    assert_eq!(snapshot.version(), BufferVersion::INITIAL);
    assert_eq!(snapshot.line_count(), 2);
    assert!(!snapshot.is_stale_for(&buffer));
    assert!(!buffer.is_snapshot_stale(&snapshot));

    let joined = thread::spawn({
        let snapshot = snapshot.clone();
        move || {
            (
                snapshot.version(),
                snapshot.text().to_string(),
                snapshot.line_count(),
            )
        }
    })
    .join()
    .unwrap();

    assert_eq!(
        joined,
        (BufferVersion::INITIAL, "line1\nline2".to_string(), 2)
    );

    buffer.insert(ByteOffset::new(0), "> ").unwrap();

    assert_eq!(snapshot.text(), "line1\nline2");
    assert!(snapshot.is_stale_for(&buffer));
    assert!(buffer.is_version_stale(snapshot.version()));
}

#[test]
fn undo_and_redo_on_empty_history_are_noops() {
    let mut buffer = buffer("stable");

    assert!(buffer.undo().unwrap().is_none());
    assert!(buffer.redo().unwrap().is_none());
    assert_eq!(buffer.text(), "stable");
    assert_eq!(buffer.version(), BufferVersion::INITIAL);
}
