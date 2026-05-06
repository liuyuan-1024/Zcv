//! M9B 机器契约：锁定 TrackedRange 的 Anchor 边界、stickiness、失效与批量更新语义。
//!
//! 本文件只验证 TrackedRange public API，不测试 metadata layer、fold projection 或 UI testbed。

use zom_engine::{
    Affinity, Anchor, AnchorError, Buffer, BufferConfig, BufferVersion, CharOffset, Edit,
    EngineError, FoldedRange, MappingResult, Stickiness, TextRange, TrackedRange,
    TrackedRangeCollapsePolicy, TrackedRangeInvalidationPolicy, TrackedRangeUpdate,
    TrackedRangeUpdatePolicy, Transaction,
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

fn tracked(
    version: BufferVersion,
    start: usize,
    end: usize,
    stickiness: Stickiness,
) -> TrackedRange {
    TrackedRange::from_range(version, range(start, end), stickiness)
}

fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> zom_engine::DeltaEvent {
    let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
    buffer.apply_transaction(tx).unwrap();
    buffer.last_delta_event().unwrap().clone()
}

#[test]
fn tracked_range_is_expressed_by_two_versioned_anchors() {
    let tracked = tracked(BufferVersion::INITIAL, 1, 3, Stickiness::Expand);

    assert_eq!(tracked.version(), BufferVersion::INITIAL);
    assert_eq!(tracked.range(), range(1, 3));
    assert_eq!(tracked.stickiness(), Stickiness::Expand);
    assert_eq!(tracked.start_anchor().offset(), c(1));
    assert_eq!(tracked.start_anchor().affinity(), Affinity::Before);
    assert_eq!(tracked.end_anchor().offset(), c(3));
    assert_eq!(tracked.end_anchor().affinity(), Affinity::After);
}

#[test]
fn tracked_range_constructor_rejects_mismatched_anchor_versions_and_reversed_range() {
    let err = TrackedRange::new(
        Anchor::new(BufferVersion::INITIAL, c(1)),
        Anchor::new(BufferVersion::new(1), c(3)),
        Stickiness::Never,
    )
    .unwrap_err();

    assert_eq!(
        err,
        EngineError::Anchor(AnchorError::RangeVersionMismatch {
            start: BufferVersion::INITIAL,
            end: BufferVersion::new(1),
        })
    );

    let err = TrackedRange::new(
        Anchor::new(BufferVersion::INITIAL, c(3)),
        Anchor::new(BufferVersion::INITIAL, c(1)),
        Stickiness::Never,
    )
    .unwrap_err();

    assert!(matches!(err, EngineError::Coordinate(_)));
}

#[test]
fn stickiness_controls_growth_at_insert_boundaries() {
    let mut buffer = buffer("abcd");
    let version = buffer.version();
    let event = apply(
        &mut buffer,
        vec![
            Edit::insert(c(1), "X".to_string()).unwrap(),
            Edit::insert(c(3), "Y".to_string()).unwrap(),
        ],
    );

    let never = tracked(version, 1, 3, Stickiness::Never);
    let expand = tracked(version, 1, 3, Stickiness::Expand);
    let before = tracked(version, 1, 3, Stickiness::BeforeInsertion);
    let after = tracked(version, 1, 3, Stickiness::AfterInsertion);

    assert_eq!(
        never.map_through_delta_event(&event),
        Ok(MappingResult::Mapped(tracked(
            buffer.version(),
            2,
            4,
            Stickiness::Never
        )))
    );
    assert_eq!(
        expand.map_through_delta_event(&event),
        Ok(MappingResult::Mapped(tracked(
            buffer.version(),
            1,
            5,
            Stickiness::Expand
        )))
    );
    assert_eq!(
        before.map_through_delta_event(&event),
        Ok(MappingResult::Mapped(tracked(
            buffer.version(),
            1,
            4,
            Stickiness::BeforeInsertion
        )))
    );
    assert_eq!(
        after.map_through_delta_event(&event),
        Ok(MappingResult::Mapped(tracked(
            buffer.version(),
            2,
            5,
            Stickiness::AfterInsertion
        )))
    );
}

#[test]
fn fully_deleted_range_can_collapse_or_invalidate() {
    let mut buffer = buffer("abcdef");
    let tracked_range = tracked(buffer.version(), 1, 4, Stickiness::Never);
    let event = apply(&mut buffer, vec![Edit::delete(range(1, 4))]);

    assert_eq!(
        tracked_range.map_through_delta_event(&event),
        Ok(MappingResult::Collapsed(tracked(
            buffer.version(),
            1,
            1,
            Stickiness::Never
        )))
    );
    assert_eq!(
        tracked_range.map_through_delta_event_with_policy(
            &event,
            TrackedRangeUpdatePolicy::invalidate_when_fully_deleted()
        ),
        Ok(TrackedRangeUpdate::Invalidated {
            range: range(1, 1),
            version: buffer.version(),
        })
    );
    assert_eq!(
        tracked_range.map_through_delta_event_with_policy(
            &event,
            TrackedRangeUpdatePolicy::new(
                TrackedRangeInvalidationPolicy::Never,
                TrackedRangeCollapsePolicy::Invalidate,
            )
        ),
        Ok(TrackedRangeUpdate::Invalidated {
            range: range(1, 1),
            version: buffer.version(),
        })
    );
}

#[test]
fn partially_deleted_range_shrinks_by_default_or_invalidates_when_requested() {
    let mut buffer = buffer("abcdef");
    let tracked_range = tracked(buffer.version(), 1, 5, Stickiness::Never);
    let event = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);

    assert_eq!(buffer.text().as_ref(), "abef");
    assert_eq!(
        tracked_range.map_through_delta_event(&event),
        Ok(MappingResult::Deleted(tracked(
            buffer.version(),
            1,
            3,
            Stickiness::Never
        )))
    );
    assert_eq!(
        tracked_range
            .map_through_delta_event_with_policy(&event, TrackedRangeUpdatePolicy::default()),
        Ok(TrackedRangeUpdate::Deleted(tracked(
            buffer.version(),
            1,
            3,
            Stickiness::Never
        )))
    );
    assert_eq!(
        tracked_range.map_through_delta_event_with_policy(
            &event,
            TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion()
        ),
        Ok(TrackedRangeUpdate::Invalidated {
            range: range(1, 3),
            version: buffer.version(),
        })
    );
}

#[test]
fn tracked_ranges_can_be_updated_in_batch_without_partial_version_mismatch_mutation() {
    let mut buffer = buffer("abcdef");
    let mut ranges = vec![
        tracked(buffer.version(), 1, 3, Stickiness::Never),
        tracked(buffer.version(), 3, 6, Stickiness::Expand),
    ];
    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(3), "XYZ".to_string()).unwrap()],
    );

    let updates = TrackedRange::update_all_through_delta_event(&mut ranges, &event).unwrap();

    assert_eq!(
        updates,
        vec![
            MappingResult::Mapped(tracked(buffer.version(), 1, 3, Stickiness::Never)),
            MappingResult::Mapped(tracked(buffer.version(), 3, 9, Stickiness::Expand)),
        ]
    );
    assert_eq!(
        ranges,
        vec![
            tracked(buffer.version(), 1, 3, Stickiness::Never),
            tracked(buffer.version(), 3, 9, Stickiness::Expand),
        ]
    );

    let before_failed_update = ranges.clone();
    let mut other_buffer = Buffer::from_text("xyz".to_string(), BufferConfig::default()).unwrap();
    let unrelated_event = apply(&mut other_buffer, vec![Edit::delete(range(0, 1))]);
    let err =
        TrackedRange::update_all_through_delta_event(&mut ranges, &unrelated_event).unwrap_err();

    assert_eq!(
        err,
        AnchorError::VersionMismatch {
            expected: BufferVersion::INITIAL,
            actual: BufferVersion::new(1),
        }
    );
    assert_eq!(ranges, before_failed_update);
}

#[test]
fn folded_range_reuses_tracked_range_following_math() {
    let mut buffer = buffer("abcdef");
    let mut folded: FoldedRange = tracked(buffer.version(), 1, 5, Stickiness::Expand);
    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "X".to_string()).unwrap()],
    );

    folded.update_through_delta_event(&event).unwrap();

    assert_eq!(folded.range(), range(1, 6));
    assert_eq!(folded.version(), buffer.version());
}
