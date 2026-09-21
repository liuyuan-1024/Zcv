use super::*;
use crate::{
    position_map::PositionMap,
    transaction::{ChangeSet, Delta, Edit, EditList, TransactionSource},
    types::TransactionId,
};

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn event_for_edits(
    old_version: BufferVersion,
    new_version: BufferVersion,
    edits: Vec<Edit>,
) -> DeltaEvent {
    let edit_list = EditList::new(edits).unwrap();
    let delta = Delta::new(old_version, new_version, edit_list.clone());
    let changeset = ChangeSet::from_edit_list(&edit_list);
    let position_map = PositionMap::from_edits(edit_list.as_slice());

    DeltaEvent::new(
        TransactionId::INITIAL,
        TransactionSource::Programmatic,
        delta,
        changeset,
        position_map,
    )
}

#[test]
fn anchor_should_map_through_delta_with_affinity() {
    let insert_event = event_for_edits(
        BufferVersion::INITIAL,
        BufferVersion::new(1),
        vec![Edit::insert(b(2), "XX".to_string()).unwrap()],
    );
    let anchor = Anchor::new(BufferVersion::INITIAL, b(2)).with_affinity(Affinity::Before);

    assert_eq!(
        anchor
            .map_through_delta_event(&insert_event)
            .unwrap()
            .value()
            .offset(),
        b(2)
    );
}

#[test]
fn anchor_should_follow_boundary_insertion_according_to_affinity() {
    let insert_event = event_for_edits(
        BufferVersion::INITIAL,
        BufferVersion::new(1),
        vec![Edit::insert(b(2), "XX".to_string()).unwrap()],
    );
    let after = Anchor::new(BufferVersion::INITIAL, b(2)).with_affinity(Affinity::After);

    assert_eq!(
        after
            .map_through_delta_event(&insert_event)
            .unwrap()
            .value()
            .offset(),
        b(4)
    );
}

#[test]
fn anchor_inside_deleted_text_reports_deleted_mapping() {
    let delete_event = event_for_edits(
        BufferVersion::INITIAL,
        BufferVersion::new(1),
        vec![Edit::replace(
            TextRange::new(b(2), b(4)).unwrap(),
            String::new(),
        )],
    );
    let anchor = Anchor::new(BufferVersion::INITIAL, b(3)).with_affinity(Affinity::After);

    assert!(matches!(
        anchor.map_through_delta_event(&delete_event).unwrap(),
        MappingResult::Deleted(mapped)
            if mapped.offset() == b(2) && mapped.affinity() == Affinity::After
    ));
}

#[test]
fn anchor_rejects_a_delta_from_another_snapshot_version() {
    let event = event_for_edits(
        BufferVersion::new(1),
        BufferVersion::new(2),
        vec![Edit::insert(b(0), "x".to_string()).unwrap()],
    );
    let anchor = Anchor::default();

    assert!(matches!(
        anchor.map_through_delta_event(&event),
        Err(AnchorError::VersionMismatch { .. })
    ));
}

#[test]
fn anchor_ranges_should_express_boundary_insertion_policy() {
    let range = TextRange::new(b(2), b(5)).unwrap();
    let inside = Anchor::range_inside(BufferVersion::INITIAL, range);
    let outside = Anchor::range_outside(BufferVersion::INITIAL, range);

    assert_eq!(inside.start.affinity(), Affinity::After);
    assert_eq!(inside.end.affinity(), Affinity::Before);
    assert_eq!(outside.start.affinity(), Affinity::Before);
    assert_eq!(outside.end.affinity(), Affinity::After);
}
