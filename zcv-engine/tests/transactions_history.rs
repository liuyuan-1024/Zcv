use zcv_engine::*;
mod common;
use common::*;

#[test]
fn apply_transaction_should_emit_delta_changeset_position_map_and_pending_event() {
    let mut buffer = buffer("abc def");
    let base = buffer.version();
    let transaction = tx(
        &buffer,
        vec![
            Edit::insert(b(3), "!".to_string()).unwrap(),
            Edit::replace(range(4, 7), "XYZ".to_string()),
        ],
    );

    let (delta, changeset) = buffer.apply_transaction(transaction).unwrap();
    let event = buffer.last_delta_event().unwrap();

    assert_eq!(buffer_text(&buffer), "abc! XYZ");
    assert_eq!(delta.old_version(), base);
    assert_eq!(delta.new_version(), buffer.version());
    assert_eq!(delta.edits().as_slice().len(), 2);
    assert_eq!(
        changeset.changed_ranges().unwrap(),
        vec![range(3, 4), range(5, 8)]
    );
    assert_eq!(
        changeset.position_map().map_old_position(b(7)).value(),
        b(8)
    );
    assert_eq!(event.old_version(), base);
    assert_eq!(event.new_version(), buffer.version());
    assert_eq!(event.source(), TransactionSource::Programmatic);
    assert_eq!(event.position_map().map_old_position(b(7)).value(), b(8));
    assert_eq!(buffer.pending_delta_event_count(), 1);
}

#[test]
fn stale_base_version_should_fail_without_mutating_text_version_history_or_events() {
    let mut buffer = buffer("abc");
    buffer.insert(b(3), "!").unwrap();
    buffer.take_pending_events();
    let text = buffer_text(&buffer);
    let version = buffer.version();
    let history = buffer.history_status();
    let stale = Transaction::from_edits(
        BufferVersion::INITIAL,
        vec![Edit::insert(b(0), "x".to_string()).unwrap()],
    )
    .unwrap();

    let err = buffer.apply_transaction(stale).unwrap_err();

    assert!(matches!(
        err,
        EngineError::Transaction(TransactionError::VersionMismatch { .. })
    ));
    assert_eq!(buffer_text(&buffer), text);
    assert_eq!(buffer.version(), version);
    assert_eq!(buffer.history_status().undo_depth, history.undo_depth);
    assert_eq!(buffer.pending_delta_event_count(), 0);
}

#[test]
fn failed_multi_edit_boundary_should_keep_transaction_atomic() {
    let mut buffer = buffer("a\r\nb");
    let text = buffer_text(&buffer);
    let version = buffer.version();
    let transaction = tx(
        &buffer,
        vec![
            Edit::insert(buffer.len_bytes(), "!".to_string()).unwrap(),
            Edit::insert(b(2), "x".to_string()).unwrap(),
        ],
    );

    let err = buffer.apply_transaction(transaction).unwrap_err();

    assert!(matches!(
        err,
        EngineError::Edit(EditError::InvalidBoundary { offset }) if offset == b(2)
    ));
    assert_eq!(buffer_text(&buffer), text);
    assert_eq!(buffer.version(), version);
}

#[test]
fn transaction_record_should_replay_only_on_matching_base_version() {
    let mut source = buffer("abc");
    let record = source
        .apply_transaction_recorded(tx(
            &source,
            vec![Edit::insert(b(3), "!".to_string()).unwrap()],
        ))
        .unwrap();
    let mut target = buffer("abc");

    let replay = target.replay_transaction_record(&record).unwrap();

    assert_eq!(buffer_text(&target), "abc!");
    assert_eq!(replay.old_version(), BufferVersion::INITIAL);
    assert_eq!(replay.new_version(), target.version());
    assert_eq!(replay.edits().as_slice(), record.edits().as_slice());

    let err = target.replay_transaction_record(&record).unwrap_err();
    assert!(matches!(
        err,
        EngineError::Transaction(TransactionError::VersionMismatch { .. })
    ));
}

#[test]
fn undo_redo_should_restore_text_selection_and_dirty_state() {
    let mut buffer = buffer("abc");
    buffer.set_selection(SelectionSet::caret(b(1))).unwrap();
    buffer
        .insert_at_selections(buffer.selection().clone(), "X", metadata("insert"))
        .unwrap();
    buffer.mark_saved();
    buffer.insert(b(4), "!").unwrap();

    assert_eq!(buffer_text(&buffer), "aXbc!");
    assert!(buffer.is_dirty());
    assert!(buffer.can_undo());

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "aXbc");
    assert!(!buffer.is_dirty());

    buffer.redo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "aXbc!");
    assert!(buffer.is_dirty());
}

#[test]
fn explicit_history_merge_should_merge_selection_edits_into_one_undo_step() {
    let mut buffer = buffer("");

    for (index, text) in ["a", "b", "c"].into_iter().enumerate() {
        let metadata = if index == 0 {
            metadata("insert")
        } else {
            merge_metadata("insert")
        };
        buffer
            .insert_at_selections(buffer.selection().clone(), text, metadata)
            .unwrap();
    }

    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(buffer.history_status().undo_depth, 1);

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "");
    assert_eq!(buffer.selection().ranges(), vec![range(0, 0)]);

    buffer.redo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(buffer.selection().ranges(), vec![range(3, 3)]);
}

#[test]
fn default_selection_edits_should_stay_separate() {
    let mut buffer = buffer("");

    buffer
        .insert_at_selections(buffer.selection().clone(), "a", metadata("insert"))
        .unwrap();
    buffer
        .delete_at_selections(
            buffer.selection().clone(),
            Some((MovementDirection::Previous, MovementUnit::Grapheme)),
            metadata("delete"),
        )
        .unwrap();

    assert_eq!(buffer_text(&buffer), "");
    assert_eq!(buffer.history_status().undo_depth, 2);

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "a");

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "");
}

#[test]
fn branch_history_should_expose_redo_branches_and_replay_selected_branch() {
    let mut buffer = buffer("a");

    buffer.insert(b(1), "b").unwrap();
    buffer.undo().unwrap().unwrap();
    buffer.insert(b(1), "c").unwrap();
    buffer.undo().unwrap().unwrap();

    let branches = buffer.redo_branches();
    assert_eq!(branches.len(), 2);

    buffer.redo_to_branch(branches[0]).unwrap();
    assert!(matches!(buffer_text(&buffer).as_str(), "ab" | "ac"));
}

#[test]
fn large_transaction_reject_policy_should_preserve_history_and_state() {
    let mut buffer = Buffer::from_text(
        "abc".to_string(),
        BufferConfig {
            large_file: LargeFilePolicy {
                large_transaction_threshold_bytes: 2,
                large_transaction_policy: LargeTransactionPolicy::Reject,
                ..LargeFilePolicy::default()
            },
            ..BufferConfig::default()
        },
    )
    .unwrap();
    let version = buffer.version();

    let err = buffer.insert(b(3), "long").unwrap_err();

    assert!(matches!(
        err,
        EngineError::Edit(EditError::PayloadTooLarge { size, limit }) if size > limit
    ));
    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(buffer.version(), version);
    assert_eq!(buffer.history_status().undo_depth, 0);
}
