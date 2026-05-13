use zom_engine::*;

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn tx(buffer: &Buffer, edits: Vec<Edit>) -> Transaction {
    Transaction::from_edits(buffer.version(), edits).unwrap()
}

#[test]
fn edit_list_should_sort_adjacent_edits_and_reject_overlap() {
    let later = Edit::replace(range(4, 5), "Y".to_string());
    let earlier = Edit::replace(range(0, 1), "X".to_string());
    let sorted = EditList::new(vec![later, earlier]).unwrap();

    assert_eq!(sorted.as_slice()[0].range(), range(0, 1));
    assert_eq!(sorted.as_slice()[1].range(), range(4, 5));

    let err = EditList::new(vec![
        Edit::replace(range(0, 3), "a".to_string()),
        Edit::replace(range(2, 4), "b".to_string()),
    ])
    .unwrap_err();
    assert!(matches!(err, EditError::OverlappingEdits { .. }));
}

#[test]
fn transaction_empty_edit_list_should_be_rejected_before_state_transition() {
    let err =
        Transaction::new(BufferVersion::INITIAL, EditList::new(Vec::new()).unwrap()).unwrap_err();

    assert_eq!(err, TransactionError::EmptyTransaction);
}

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

    assert_eq!(buffer.text().as_ref(), "abc! XYZ");
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
    let text = buffer.text().to_string();
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
    assert_eq!(buffer.text().as_ref(), text);
    assert_eq!(buffer.version(), version);
    assert_eq!(buffer.history_status().undo_depth, history.undo_depth);
    assert_eq!(buffer.pending_delta_event_count(), 0);
}

#[test]
fn failed_multi_edit_boundary_should_keep_transaction_atomic() {
    let mut buffer = buffer("a\r\nb");
    let text = buffer.text().to_string();
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
    assert_eq!(buffer.text().as_ref(), text);
    assert_eq!(buffer.version(), version);
}

#[test]
fn position_map_should_expose_affinity_bias_stickiness_and_selection_mapping() {
    let mut buffer = buffer("abcd");
    let (_, changeset) = buffer
        .apply_transaction(tx(
            &buffer,
            vec![Edit::replace(range(1, 3), "XYZ".to_string())],
        ))
        .unwrap();
    let map = changeset.position_map();

    assert_eq!(map.len(), 1);
    assert!(matches!(map.map_old_position(b(2)), MappingResult::Deleted(pos) if pos == b(1)));
    assert!(matches!(
        map.map_new_position_with_bias(b(2), Bias::Right),
        MappingResult::Ambiguous(pos) if pos == b(3)
    ));
    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 3), Stickiness::Expand)
            .value(),
        range(1, 4)
    );
    assert_eq!(
        Selection::new(b(0), b(4)).map_through_position_map(&map),
        Selection::new(b(0), b(5))
    );
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

    assert_eq!(target.text().as_ref(), "abc!");
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
        .insert_at_selections(buffer.selection().clone(), "X")
        .unwrap();
    buffer.mark_saved();
    buffer.insert(b(4), "!").unwrap();

    assert_eq!(buffer.text().as_ref(), "aXbc!");
    assert!(buffer.is_dirty());
    assert!(buffer.can_undo());

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer.text().as_ref(), "aXbc");
    assert!(!buffer.is_dirty());

    buffer.redo().unwrap().unwrap();
    assert_eq!(buffer.text().as_ref(), "aXbc!");
    assert!(buffer.is_dirty());
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
    assert!(matches!(buffer.text().as_ref(), "ab" | "ac"));
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
    assert_eq!(buffer.text().as_ref(), "abc");
    assert_eq!(buffer.version(), version);
    assert_eq!(buffer.history_status().undo_depth, 0);
}
