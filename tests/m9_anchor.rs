//! M9A 机器契约：锁定 Anchor / Mark 的版本绑定、Affinity 与 PositionMap 跟随语义。
//!
//! 本文件只验证 Anchor / Mark public API，不测试 TrackedRange、metadata layer 或 UI testbed。

use zom_engine::{
    Affinity, Anchor, AnchorDeletedPolicy, AnchorError, AnchorUpdate, Buffer, BufferConfig,
    BufferVersion, CharOffset, Edit, MappingResult, Mark, TextRange, Transaction,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(c(start), c(end)).unwrap()
}

fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> zom_engine::DeltaEvent {
    let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
    buffer.apply_transaction(tx).unwrap();
    buffer.last_delta_event().unwrap().clone()
}

#[test]
fn anchor_binds_position_to_buffer_version_and_affinity() {
    let buffer = buffer("ab");
    let anchor = Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::Before);

    assert_eq!(anchor.version(), BufferVersion::INITIAL);
    assert_eq!(anchor.offset(), c(1));
    assert_eq!(anchor.affinity(), Affinity::Before);
    assert_eq!(
        anchor.to_mark(),
        Mark::new(c(1)).with_affinity(Affinity::Before)
    );
}

#[test]
fn anchor_maps_through_delta_event_with_affinity() {
    let mut buffer = buffer("ab");
    let before = Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::Before);
    let after = Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::After);

    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    assert_eq!(
        before.map_through_delta_event(&event),
        Ok(MappingResult::Mapped(
            Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::Before)
        ))
    );
    assert_eq!(
        after.map_through_delta_event(&event),
        Ok(MappingResult::Mapped(
            Anchor::new(buffer.version(), c(4)).with_affinity(Affinity::After)
        ))
    );
}

#[test]
fn mark_maps_as_lightweight_unversioned_position() {
    let mut buffer = buffer("ab");
    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    let before = Mark::new(c(1)).with_affinity(Affinity::Before);
    let after = Mark::new(c(1)).with_affinity(Affinity::After);

    assert_eq!(
        before.map_through_position_map(&event.position_map),
        MappingResult::Mapped(Mark::new(c(1)).with_affinity(Affinity::Before))
    );
    assert_eq!(
        after.map_through_position_map(&event.position_map),
        MappingResult::Mapped(Mark::new(c(4)).with_affinity(Affinity::After))
    );
}

#[test]
fn anchor_rejects_mapping_through_unrelated_version_event() {
    let mut buffer = buffer("abc");
    let stale = Anchor::new(BufferVersion::new(99), c(1));
    let event = apply(&mut buffer, vec![Edit::delete(range(0, 1))]);

    assert_eq!(
        stale.map_through_delta_event(&event),
        Err(AnchorError::VersionMismatch {
            expected: BufferVersion::INITIAL,
            actual: BufferVersion::new(99),
        })
    );
}

#[test]
fn deleted_anchor_can_collapse_or_invalidate() {
    let mut buffer = buffer("abcdef");
    let anchor = Anchor::new(buffer.version(), c(2));
    let event = apply(&mut buffer, vec![Edit::delete(range(1, 4))]);

    assert_eq!(
        anchor.map_through_delta_event(&event),
        Ok(MappingResult::Deleted(Anchor::new(buffer.version(), c(1))))
    );
    assert_eq!(
        anchor.map_through_delta_event_with_deleted_policy(&event, AnchorDeletedPolicy::Collapse),
        Ok(AnchorUpdate::Deleted(Anchor::new(buffer.version(), c(1))))
    );
    assert_eq!(
        anchor.map_through_delta_event_with_deleted_policy(&event, AnchorDeletedPolicy::Invalidate),
        Ok(AnchorUpdate::Invalidated {
            mark: Mark::new(c(1)),
            version: buffer.version(),
        })
    );
}

#[test]
fn anchors_can_be_updated_in_batch_without_partial_version_mismatch_mutation() {
    let mut buffer = buffer("abc");
    let mut anchors = vec![
        Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::Before),
        Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::After),
    ];
    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    let updates = Anchor::update_all_through_delta_event(&mut anchors, &event).unwrap();

    assert_eq!(
        updates,
        vec![
            MappingResult::Mapped(
                Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::Before)
            ),
            MappingResult::Mapped(
                Anchor::new(buffer.version(), c(4)).with_affinity(Affinity::After)
            ),
        ]
    );
    assert_eq!(
        anchors,
        vec![
            Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::Before),
            Anchor::new(buffer.version(), c(4)).with_affinity(Affinity::After),
        ]
    );

    let before_failed_update = anchors.clone();
    let mut other_buffer = Buffer::from_text("xyz".to_string(), BufferConfig::default()).unwrap();
    let unrelated_event = apply(&mut other_buffer, vec![Edit::delete(range(0, 1))]);
    let err = Anchor::update_all_through_delta_event(&mut anchors, &unrelated_event).unwrap_err();

    assert_eq!(
        err,
        AnchorError::VersionMismatch {
            expected: BufferVersion::INITIAL,
            actual: BufferVersion::new(1),
        }
    );
    assert_eq!(anchors, before_failed_update);
}

#[test]
fn anchors_can_be_batch_mapped_with_deleted_policy() {
    let mut buffer = buffer("abcdef");
    let anchors = vec![
        Anchor::new(buffer.version(), c(2)),
        Anchor::new(buffer.version(), c(5)),
    ];
    let event = apply(&mut buffer, vec![Edit::delete(range(1, 4))]);

    let updates = Anchor::map_all_through_delta_event_with_deleted_policy(
        anchors,
        &event,
        AnchorDeletedPolicy::Invalidate,
    )
    .unwrap();

    assert_eq!(
        updates,
        vec![
            AnchorUpdate::Invalidated {
                mark: Mark::new(c(1)),
                version: buffer.version(),
            },
            AnchorUpdate::Mapped(Anchor::new(buffer.version(), c(2))),
        ]
    );
}
