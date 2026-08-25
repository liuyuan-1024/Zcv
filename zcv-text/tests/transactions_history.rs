use zcv_text::*;
mod common;
use common::*;

#[test]
fn edit_should_emit_delta_changeset_position_map_and_subscription_patch() {
    let mut buffer = buffer("abc def");
    let subscription = buffer.subscribe();
    let base = buffer.version();
    let outcome = buffer
        .edit(
            [
                Edit::insert(b(3), "!".to_string()).unwrap(),
                Edit::replace(range(4, 7), "XYZ".to_string()),
            ],
            TransactionMetadata::default(),
        )
        .unwrap();
    let event = outcome.event();
    let delta = event.delta();
    let changeset = event.changeset();
    let changes = subscription.consume();

    assert_eq!(buffer_text(&buffer), "abc! XYZ");
    assert_eq!(delta.old_version(), base);
    assert_eq!(delta.new_version(), buffer.version());
    assert_eq!(delta.edits().len(), 2);
    assert_eq!(
        changeset.changed_ranges().unwrap(),
        vec![range(3, 4), range(5, 8)]
    );
    assert_eq!(event.position_map().map_old_position(b(7)).value(), b(8));
    assert_eq!(event.old_version(), base);
    assert_eq!(event.new_version(), buffer.version());
    assert_eq!(event.source(), TransactionSource::Programmatic);
    assert_eq!(event.position_map().map_old_position(b(7)).value(), b(8));
    assert_eq!(changes.old_version(), Some(base));
    assert_eq!(changes.new_version(), Some(buffer.version()));
    assert_eq!(changes.patch().edits().len(), 2);
}

#[test]
fn failed_multi_edit_boundary_should_keep_transaction_atomic() {
    let mut buffer = buffer("a\r\nb");
    let text = buffer_text(&buffer);
    let version = buffer.version();
    let err = buffer
        .edit(
            [
                Edit::insert(buffer.len_bytes(), "!".to_string()).unwrap(),
                Edit::insert(b(2), "x".to_string()).unwrap(),
            ],
            TransactionMetadata::default(),
        )
        .unwrap_err();

    assert!(matches!(
        err,
        TextError::Edit(EditError::InvalidBoundary { offset }) if offset == b(2)
    ));
    assert_eq!(buffer_text(&buffer), text);
    assert_eq!(buffer.version(), version);
}

#[test]
fn undo_redo_should_restore_text_and_dirty_state_and_return_history_identity() {
    let mut buffer = buffer("abc");
    let outcome = buffer
        .edit([Edit::insert(b(1), "X").unwrap()], metadata("insert"))
        .unwrap();
    let selection_transaction_id = outcome.history_transaction_id().unwrap();
    buffer.mark_saved();
    buffer
        .edit(
            [Edit::insert(b(4), "!").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();

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
    let mut canonical_transaction_id = None;

    for (index, text) in ["a", "b", "c"].into_iter().enumerate() {
        let metadata = if index == 0 {
            metadata("insert")
        } else {
            merge_metadata("insert")
        };
        let outcome = buffer
            .edit([Edit::insert(buffer.len_bytes(), text).unwrap()], metadata)
            .unwrap();
        let history_transaction_id = outcome.history_transaction_id().unwrap();
        if let Some(expected) = canonical_transaction_id {
            assert_eq!(history_transaction_id, expected);
        } else {
            canonical_transaction_id = Some(history_transaction_id);
        }
    }

    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(buffer.history_status().undo_depth, 1);

    let undo = buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "");
    assert_eq!(undo.transaction_id(), canonical_transaction_id.unwrap());

    let redo = buffer.redo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(redo.transaction_id(), canonical_transaction_id.unwrap());
}

#[test]
fn default_transactions_should_stay_separate() {
    let mut buffer = buffer("");
    buffer
        .edit(
            [Edit::insert(ByteOffset::ZERO, "a").unwrap()],
            metadata("insert"),
        )
        .unwrap();
    buffer
        .edit([Edit::delete(range(0, 1))], metadata("delete"))
        .unwrap();

    assert_eq!(buffer_text(&buffer), "");
    assert_eq!(buffer.history_status().undo_depth, 2);

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "a");

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "");
}

#[test]
fn set_config_should_apply_the_new_history_budget_immediately() {
    let mut buffer = buffer("");
    buffer
        .edit(
            [Edit::insert(ByteOffset::ZERO, "a").unwrap()],
            metadata("insert"),
        )
        .unwrap();
    assert!(buffer.can_undo());

    let mut config = buffer.config().clone();
    config.large_file.max_undo_history = 0;
    buffer.set_config(config);

    assert!(!buffer.can_undo());
    assert_eq!(buffer.history_status().node_count, 0);
}

#[test]
fn transaction_should_not_report_history_identity_when_history_is_disabled() {
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
        .edit(
            [Edit::insert(ByteOffset::ZERO, "a").unwrap()],
            metadata("insert"),
        )
        .unwrap();

    assert_eq!(outcome.event().transaction_id(), TransactionId::INITIAL);
    assert!(outcome.history_transaction_id().is_none());
    assert!(!buffer.can_undo());
}

#[test]
fn branch_history_should_expose_redo_branches_and_replay_selected_branch() {
    let mut buffer = buffer("a");

    buffer
        .edit(
            [Edit::insert(b(1), "b").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    buffer.undo().unwrap().unwrap();
    buffer
        .edit(
            [Edit::insert(b(1), "c").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
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

    let err = buffer
        .edit(
            [Edit::insert(b(3), "long").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap_err();

    assert!(matches!(
        err,
        TextError::Edit(EditError::PayloadTooLarge { size, limit }) if size > limit
    ));
    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(buffer.version(), version);
    assert_eq!(buffer.history_status().undo_depth, 0);
}
