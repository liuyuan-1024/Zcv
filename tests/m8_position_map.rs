//! M8 机器契约：锁定 PositionMap、吸附策略和 DeltaEvent 队列。
//!
//! 本文件只验证 M8A～M8C public API，不测试 Anchor / TrackedRange 生命周期或 UI testbed。

use zom_engine::{
    Affinity, Bias, Buffer, BufferConfig, CharOffset, Edit, MappingResult, PositionMap, Stickiness,
    TextRange, Transaction, TransactionError, TransactionId, TransactionMetadata,
    TransactionSource,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn c(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(c(start), c(end)).unwrap()
}

fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> PositionMap {
    let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
    let (_, changeset) = buffer.apply_transaction(tx).unwrap();
    changeset.position_map()
}

#[test]
fn position_map_maps_old_positions_to_new_positions() {
    let mut buffer = buffer("12345");
    let map = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);

    assert_eq!(buffer.text(), "125");
    assert_eq!(map.map_old_position(c(0)), MappingResult::Mapped(c(0)));
    assert_eq!(map.map_old_position(c(2)), MappingResult::Deleted(c(2)));
    assert_eq!(map.map_old_position(c(3)), MappingResult::Deleted(c(2)));
    assert_eq!(map.map_old_position(c(5)), MappingResult::Mapped(c(3)));
}

#[test]
fn position_map_maps_new_positions_back_to_old_positions_with_ambiguity() {
    let mut buffer = buffer("ab");
    let map = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    assert_eq!(buffer.text(), "aXYZb");
    assert_eq!(map.map_old_position(c(1)), MappingResult::Mapped(c(4)));
    assert_eq!(map.map_new_position(c(0)), MappingResult::Mapped(c(0)));
    assert_eq!(map.map_new_position(c(2)), MappingResult::Ambiguous(c(1)));
    assert_eq!(map.map_new_position(c(4)), MappingResult::Mapped(c(1)));
    assert_eq!(map.map_new_position(c(5)), MappingResult::Mapped(c(2)));
}

#[test]
fn affinity_controls_old_position_at_insert_boundary() {
    let mut buffer = buffer("ab");
    let map = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    assert_eq!(
        map.map_old_position_with_affinity(c(1), Affinity::Before),
        MappingResult::Mapped(c(1))
    );
    assert_eq!(
        map.map_old_position_with_affinity(c(1), Affinity::After),
        MappingResult::Mapped(c(4))
    );
}

#[test]
fn bias_controls_new_position_inside_replacement_ambiguity() {
    let mut buffer = buffer("abcdef");
    let map = apply(
        &mut buffer,
        vec![Edit::replace(range(1, 3), "XYZ".to_string())],
    );

    assert_eq!(
        map.map_new_position_with_bias(c(2), Bias::Left),
        MappingResult::Ambiguous(c(1))
    );
    assert_eq!(
        map.map_new_position_with_bias(c(2), Bias::Right),
        MappingResult::Ambiguous(c(3))
    );
}

#[test]
fn old_range_fully_deleted_maps_to_collapsed_range() {
    let mut buffer = buffer("12345");
    let map = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);

    assert_eq!(
        map.map_old_range(range(2, 4)),
        MappingResult::Collapsed(range(2, 2))
    );
}

#[test]
fn new_ranges_touching_deleted_boundary_do_not_all_become_ambiguous() {
    let mut buffer = buffer("12345");
    let map = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);

    assert_eq!(
        map.map_new_range(range(0, 2)),
        MappingResult::Mapped(range(0, 2))
    );
    assert_eq!(
        map.map_new_range(range(2, 3)),
        MappingResult::Mapped(range(4, 5))
    );
    assert_eq!(
        map.map_new_range(range(2, 2)),
        MappingResult::Ambiguous(range(2, 2))
    );
}

#[test]
fn stickiness_controls_range_growth_at_insert_boundaries() {
    let mut buffer = buffer("abcd");
    let map = apply(
        &mut buffer,
        vec![
            Edit::insert(c(1), "X".to_string()).unwrap(),
            Edit::insert(c(3), "Y".to_string()).unwrap(),
        ],
    );

    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 3), Stickiness::Never),
        MappingResult::Mapped(range(2, 4))
    );
    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 3), Stickiness::Expand),
        MappingResult::Mapped(range(1, 5))
    );
    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 3), Stickiness::BeforeInsertion),
        MappingResult::Mapped(range(1, 4))
    );
    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 3), Stickiness::AfterInsertion),
        MappingResult::Mapped(range(2, 5))
    );
}

#[test]
fn stickiness_can_expand_empty_range_to_inserted_text() {
    let mut buffer = buffer("ab");
    let map = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 1), Stickiness::Expand),
        MappingResult::Mapped(range(1, 4))
    );
    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 1), Stickiness::BeforeInsertion),
        MappingResult::Mapped(range(1, 1))
    );
    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 1), Stickiness::AfterInsertion),
        MappingResult::Mapped(range(4, 4))
    );
}

#[test]
fn replacement_reports_deleted_old_range_and_ambiguous_new_range() {
    let mut buffer = buffer("abcdef");
    let map = apply(
        &mut buffer,
        vec![Edit::replace(range(1, 3), "XYZ".to_string())],
    );

    assert_eq!(buffer.text(), "aXYZdef");
    assert_eq!(
        map.map_old_range(range(1, 3)),
        MappingResult::Deleted(range(1, 4))
    );
    assert_eq!(
        map.map_new_range(range(1, 4)),
        MappingResult::Ambiguous(range(1, 3))
    );
}

#[test]
fn old_range_expands_across_insertions_without_becoming_deleted() {
    let mut buffer = buffer("ab");
    let map = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    assert_eq!(
        map.map_old_range(range(0, 2)),
        MappingResult::Mapped(range(0, 5))
    );
}

#[test]
fn changeset_and_position_map_interoperate_without_changeset_mapping_api() {
    let mut buffer = buffer("abcdef");
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::replace(range(1, 3), "XYZ".to_string())],
    )
    .unwrap();

    let (_, changeset) = buffer.apply_transaction(tx).unwrap();
    let map = PositionMap::from_change_set(&changeset);

    assert_eq!(map.len(), 1);
    assert_eq!(map.map_old_position(c(1)), MappingResult::Deleted(c(1)));
    assert_eq!(map.map_old_position(c(3)), MappingResult::Mapped(c(4)));
}

#[test]
fn successful_transaction_enqueues_delta_event() {
    let mut buffer = buffer("hello");
    assert!(buffer.last_delta_event().is_none());
    assert!(buffer.take_pending_events().is_empty());

    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(5), " world".to_string()).unwrap()],
    )
    .unwrap()
    .with_metadata(TransactionMetadata::new(TransactionSource::Programmatic));

    let (delta, changeset) = buffer.apply_transaction(tx).unwrap();

    let last_event = buffer.last_delta_event().unwrap().clone();
    assert_eq!(last_event.transaction_id, TransactionId::INITIAL);
    assert_eq!(last_event.old_version, delta.old_version);
    assert_eq!(last_event.new_version, delta.new_version);
    assert_eq!(last_event.source, TransactionSource::Programmatic);
    assert_eq!(last_event.delta, delta);
    assert_eq!(last_event.changeset, changeset);
    assert_eq!(
        last_event.position_map.map_old_position(c(5)).value(),
        c(11)
    );

    let events = buffer.take_pending_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], last_event);
    assert_eq!(buffer.pending_delta_event_count(), 0);
    assert!(buffer.take_pending_events().is_empty());
    assert_eq!(buffer.last_delta_event(), Some(&events[0]));
}

#[test]
fn failed_transaction_does_not_enqueue_delta_event() {
    let mut buffer = buffer("hello");

    let stale_tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(5), "!".to_string()).unwrap()],
    )
    .unwrap();
    buffer.insert(c(5), "?").unwrap();
    buffer.take_pending_events();

    let err = buffer.apply_transaction(stale_tx).unwrap_err();

    assert!(matches!(
        err,
        zom_engine::EngineError::Transaction(TransactionError::VersionMismatch { .. })
    ));
    assert!(buffer.take_pending_events().is_empty());
}

#[test]
fn delta_events_preserve_transaction_and_version_order() {
    let mut buffer = buffer("ab");

    buffer.insert(c(1), "X").unwrap();
    buffer.insert(c(2), "Y").unwrap();

    assert_eq!(buffer.pending_delta_event_count(), 2);
    let events = buffer.take_pending_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].transaction_id, TransactionId::INITIAL);
    assert_eq!(events[1].transaction_id, TransactionId::new(1));
    assert_eq!(events[0].new_version, events[1].old_version);
    assert!(events[0].new_version < events[1].new_version);
}

#[test]
fn undo_and_redo_enqueue_delta_events_with_history_sources() {
    let mut buffer = buffer("ab");
    buffer.insert(c(2), "c").unwrap();
    buffer.take_pending_events();

    buffer.undo().unwrap().unwrap();
    let undo_events = buffer.take_pending_events();
    assert_eq!(undo_events.len(), 1);
    assert_eq!(undo_events[0].source, TransactionSource::Undo);
    assert_eq!(undo_events[0].delta.old_version, undo_events[0].old_version);
    assert_eq!(undo_events[0].delta.new_version, undo_events[0].new_version);
    assert_eq!(buffer.text(), "ab");

    buffer.redo().unwrap().unwrap();
    let redo_events = buffer.take_pending_events();
    assert_eq!(redo_events.len(), 1);
    assert_eq!(redo_events[0].source, TransactionSource::Redo);
    assert_eq!(redo_events[0].old_version, undo_events[0].new_version);
    assert_eq!(buffer.text(), "abc");
}
