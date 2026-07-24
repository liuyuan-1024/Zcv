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
fn undo_redo_should_restore_text_and_dirty_state_and_return_history_identity() {
    let mut buffer = buffer("abc");
    let outcome = buffer
        .insert_at_selections(&SelectionSet::caret(b(1)), "X", metadata("insert"))
        .unwrap();
    let selection_transaction_id = outcome.history_transaction_id().unwrap();
    buffer.mark_saved();
    buffer.insert(b(4), "!").unwrap();

    assert_eq!(buffer_text(&buffer), "aXbc!");
    assert!(buffer.is_dirty());
    assert!(buffer.can_undo());

    let undo = buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "aXbc");
    assert!(!buffer.is_dirty());
    assert_ne!(undo.transaction_id(), selection_transaction_id);

    let redo = buffer.redo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "aXbc!");
    assert!(buffer.is_dirty());
    assert_eq!(redo.transaction_id(), undo.transaction_id());
}

#[test]
fn explicit_history_merge_should_return_one_canonical_identity_for_editor_selection_history() {
    let mut buffer = buffer("");
    let mut selections = SelectionSet::default();
    let mut canonical_transaction_id = None;

    for (index, text) in ["a", "b", "c"].into_iter().enumerate() {
        let metadata = if index == 0 {
            metadata("insert")
        } else {
            merge_metadata("insert")
        };
        let outcome = buffer
            .insert_at_selections(&selections, text, metadata)
            .unwrap();
        let history_transaction_id = outcome.history_transaction_id().unwrap();
        if let Some(expected) = canonical_transaction_id {
            assert_eq!(history_transaction_id, expected);
        } else {
            canonical_transaction_id = Some(history_transaction_id);
        }
        selections = outcome.into_after_selections();
    }

    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(buffer.history_status().undo_depth, 1);
    assert_eq!(selections.ranges(), vec![range(3, 3)]);

    let undo = buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "");
    assert_eq!(undo.transaction_id(), canonical_transaction_id.unwrap());

    let redo = buffer.redo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(redo.transaction_id(), canonical_transaction_id.unwrap());
}

#[test]
fn default_selection_edits_should_stay_separate() {
    let mut buffer = buffer("");
    let mut selections = SelectionSet::default();

    let outcome = buffer
        .insert_at_selections(&selections, "a", metadata("insert"))
        .unwrap();
    selections = outcome.into_after_selections();
    let outcome = buffer
        .delete_at_selections(
            &selections,
            Some((MovementDirection::Previous, MovementUnit::Grapheme)),
            metadata("delete"),
        )
        .unwrap();
    selections = outcome.into_after_selections();

    assert_eq!(buffer_text(&buffer), "");
    assert_eq!(buffer.history_status().undo_depth, 2);
    assert_eq!(selections.ranges(), vec![range(0, 0)]);

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "a");

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "");
}

#[test]
fn selection_edit_should_not_report_history_identity_when_history_is_disabled() {
    let mut buffer = Buffer::from_text(
        String::new(),
        BufferConfig {
            large_file: LargeFilePolicy {
                max_undo_history: 0,
                ..LargeFilePolicy::default()
            },
            ..BufferConfig::default()
        },
    )
    .unwrap();

    let outcome = buffer
        .insert_at_selections(&SelectionSet::default(), "a", metadata("insert"))
        .unwrap();

    assert!(outcome.transaction_id().is_some());
    assert!(outcome.history_transaction_id().is_none());
    assert!(!buffer.can_undo());
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
