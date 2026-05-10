//! M14B 机器契约：锁定 `VersionedRangeSet<T>` 的版本绑定、DeltaEvent 推进、
//! 删除策略、TextRange / LineRange / line window 查询，以及与 `MetadataLayer<T>` 的互转。

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, CoordinateError, DeltaEvent, Edit,
    EngineError, Line, LineRange, MetadataLayer, MetadataLayerKind, MetadataLineWindow, Stickiness,
    TextRange, TrackedRangeCollapsePolicy, TrackedRangeInvalidationPolicy, TrackedRangeUpdate,
    TrackedRangeUpdatePolicy, Transaction, VersionedRangeSet, VersionedRangeSpec,
    VersionedResultError,
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

fn line_range(start: usize, end: usize) -> LineRange {
    LineRange::new(Line::new(start), Line::new(end)).unwrap()
}

fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> DeltaEvent {
    let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
    buffer.apply_transaction(tx).unwrap();
    buffer.last_delta_event().unwrap().clone()
}

#[test]
fn new_set_is_empty_and_binds_version() {
    let set: VersionedRangeSet<&str> = VersionedRangeSet::new(BufferVersion::new(3));

    assert_eq!(set.version(), BufferVersion::new(3));
    assert!(set.is_empty());
    assert_eq!(set.len(), 0);
    assert_eq!(set.default_stickiness(), Stickiness::default());
    assert_eq!(
        set.default_update_policy(),
        TrackedRangeUpdatePolicy::default()
    );
    assert!(!set.is_stale(BufferVersion::new(3)));
    assert!(set.is_stale(BufferVersion::new(4)));
}

#[test]
fn insert_appends_entry_with_default_policies() {
    let mut set = VersionedRangeSet::<&str>::new(BufferVersion::INITIAL)
        .with_default_stickiness(Stickiness::Expand)
        .with_default_update_policy(
            TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion(),
        );
    let first = set.insert(range(1, 3), "alpha");
    let second = set.insert_with_stickiness(range(4, 4), Stickiness::Never, "beta");
    let third = set.insert_with_options(
        range(5, 7),
        Stickiness::Never,
        TrackedRangeUpdatePolicy::invalidate_when_collapsed(),
        "gamma",
    );

    assert_eq!(first, 0);
    assert_eq!(second, 1);
    assert_eq!(third, 2);
    assert_eq!(set.len(), 3);

    let alpha = set.entry(0).unwrap();
    assert_eq!(alpha.range(), range(1, 3));
    assert_eq!(alpha.stickiness(), Stickiness::Expand);
    assert_eq!(
        alpha.update_policy(),
        TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion()
    );
    assert_eq!(alpha.payload(), &"alpha");

    let beta = set.entry(1).unwrap();
    assert_eq!(beta.stickiness(), Stickiness::Never);
    assert_eq!(
        beta.update_policy(),
        TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion()
    );

    let gamma = set.entry(2).unwrap();
    assert_eq!(gamma.stickiness(), Stickiness::Never);
    assert_eq!(
        gamma.update_policy(),
        TrackedRangeUpdatePolicy::invalidate_when_collapsed()
    );
}

#[test]
fn remove_and_clear_drop_entries() {
    let mut set = VersionedRangeSet::new(BufferVersion::INITIAL);
    set.insert(range(0, 1), 1u32);
    set.insert(range(2, 3), 2u32);
    set.insert(range(4, 5), 3u32);

    let removed = set.remove(1).unwrap();
    assert_eq!(removed.into_payload(), 2);
    assert_eq!(set.len(), 2);
    assert_eq!(set.entry(0).unwrap().payload(), &1);
    assert_eq!(set.entry(1).unwrap().payload(), &3);

    assert!(set.remove(99).is_none());

    set.clear();
    assert!(set.is_empty());
}

#[test]
fn replace_all_advances_version_and_resets_entries() {
    let mut set = VersionedRangeSet::new(BufferVersion::INITIAL);
    set.insert(range(0, 1), "stale");

    set.replace_all(
        BufferVersion::new(7),
        vec![(range(2, 3), "fresh"), (range(4, 5), "newer")],
    );

    assert_eq!(set.version(), BufferVersion::new(7));
    assert_eq!(set.len(), 2);
    assert_eq!(set.entry(0).unwrap().payload(), &"fresh");
    assert_eq!(set.entry(1).unwrap().payload(), &"newer");
}

#[test]
fn replace_all_with_options_carries_per_entry_policy() {
    let mut set = VersionedRangeSet::new(BufferVersion::INITIAL);
    set.replace_all_with_options(
        BufferVersion::new(2),
        vec![
            VersionedRangeSpec::new(range(0, 2), "first")
                .with_stickiness(Stickiness::Expand)
                .with_update_policy(TrackedRangeUpdatePolicy::invalidate_when_collapsed()),
            VersionedRangeSpec::new(range(3, 5), "second").with_stickiness(Stickiness::Never),
        ],
    );

    let first = set.entry(0).unwrap();
    assert_eq!(first.stickiness(), Stickiness::Expand);
    assert_eq!(
        first.update_policy(),
        TrackedRangeUpdatePolicy::invalidate_when_collapsed()
    );
    let second = set.entry(1).unwrap();
    assert_eq!(second.stickiness(), Stickiness::Never);
    assert_eq!(second.update_policy(), TrackedRangeUpdatePolicy::default());
}

#[test]
fn update_through_delta_event_advances_tracked_ranges() {
    let mut buffer = buffer("abcdef");
    let mut set =
        VersionedRangeSet::new(buffer.version()).with_default_stickiness(Stickiness::Expand);
    set.insert(range(1, 5), "match");
    set.insert_with_stickiness(range(5, 5), Stickiness::Never, "marker");

    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );
    let updates = set.update_through_delta_event(&event).unwrap();

    assert_eq!(set.version(), buffer.version());
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].range(), range(1, 8));
    assert_eq!(updates[0].version(), buffer.version());
    assert!(matches!(updates[0], TrackedRangeUpdate::Mapped(_)));
    assert_eq!(updates[1].range(), range(8, 8));
    assert!(matches!(updates[1], TrackedRangeUpdate::Mapped(_)));
    assert_eq!(set.entry(0).unwrap().range(), range(1, 8));
    assert_eq!(set.entry(1).unwrap().range(), range(8, 8));
}

#[test]
fn update_through_delta_event_drops_invalidated_entries() {
    let mut buffer = buffer("abcdef");
    let mut set = VersionedRangeSet::new(buffer.version());
    let policy = TrackedRangeUpdatePolicy::new(
        TrackedRangeInvalidationPolicy::WhenTouchedByDeletion,
        TrackedRangeCollapsePolicy::Keep,
    );
    set.insert_with_options(range(1, 5), Stickiness::Never, policy, "doomed");
    set.insert_with_options(range(5, 5), Stickiness::Never, policy, "survivor");

    let event = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);
    let updates = set.update_through_delta_event(&event).unwrap();

    assert_eq!(updates.len(), 2);
    assert_eq!(
        updates[0],
        TrackedRangeUpdate::Invalidated {
            range: range(1, 3),
            version: buffer.version(),
        }
    );
    assert!(matches!(updates[1], TrackedRangeUpdate::Mapped(_)));
    assert_eq!(set.len(), 1);
    assert_eq!(set.entry(0).unwrap().payload(), &"survivor");
    assert_eq!(set.entry(0).unwrap().range(), range(3, 3));
}

#[test]
fn update_through_delta_event_rejects_unrelated_event_without_partial_mutation() {
    let mut buffer = buffer("abcdef");
    let mut set = VersionedRangeSet::new(BufferVersion::new(99));
    set.insert(range(1, 3), "stale");

    let event = apply(&mut buffer, vec![Edit::delete(range(1, 2))]);
    let err = set.update_through_delta_event(&event).unwrap_err();

    assert_eq!(
        err,
        VersionedResultError::VersionMismatch {
            expected: BufferVersion::INITIAL,
            actual: BufferVersion::new(99),
        }
    );
    // 拒绝时 set 仍保持原版本与原 entry。
    assert_eq!(set.version(), BufferVersion::new(99));
    assert_eq!(set.len(), 1);
    assert_eq!(set.entry(0).unwrap().range(), range(1, 3));
}

#[test]
fn entries_intersecting_filters_by_text_range() {
    let mut set = VersionedRangeSet::new(BufferVersion::INITIAL);
    set.insert(range(0, 2), "a");
    set.insert(range(3, 5), "b");
    set.insert(range(6, 8), "c");

    let payloads = set
        .entries_intersecting(range(2, 7))
        .map(|entry| *entry.payload())
        .collect::<Vec<_>>();

    assert_eq!(payloads, vec!["b", "c"]);
}

#[test]
fn entries_containing_filters_by_offset() {
    let mut set = VersionedRangeSet::new(BufferVersion::INITIAL);
    set.insert(range(0, 2), "a");
    set.insert(range(3, 5), "b");
    set.insert(range(5, 5), "marker");

    let at_three = set
        .entries_containing(c(3))
        .map(|entry| *entry.payload())
        .collect::<Vec<_>>();
    let at_five = set
        .entries_containing(c(5))
        .map(|entry| *entry.payload())
        .collect::<Vec<_>>();

    assert_eq!(at_three, vec!["b"]);
    assert_eq!(at_five, vec!["marker"]);
}

#[test]
fn entries_in_line_range_and_window_use_buffer_boundaries() {
    let buffer = buffer("aa\nbb\ncc");
    let mut set = VersionedRangeSet::new(buffer.version());
    set.insert(range(0, 2), "line0");
    set.insert(range(3, 5), "line1");
    set.insert(range(6, 8), "line2");
    set.insert(range(2, 7), "spanning");

    let lower = set
        .entries_in_line_range(&buffer, line_range(1, 3))
        .unwrap()
        .into_iter()
        .map(|entry| *entry.payload())
        .collect::<Vec<_>>();
    let visible = set
        .entries_in_line_window(
            &buffer,
            MetadataLineWindow::from_lines(Line::new(0), Line::new(2)).unwrap(),
        )
        .unwrap()
        .into_iter()
        .map(|entry| *entry.payload())
        .collect::<Vec<_>>();

    assert_eq!(lower, vec!["line1", "line2", "spanning"]);
    assert_eq!(visible, vec!["line0", "line1", "spanning"]);
}

#[test]
fn entries_in_line_range_validates_buffer_line_boundaries() {
    let buffer = buffer("aa\nbb");
    let mut set = VersionedRangeSet::new(buffer.version());
    set.insert(range(0, 2), "line0");

    let err = set
        .entries_in_line_range(&buffer, line_range(0, 3))
        .unwrap_err();

    assert_eq!(
        err,
        EngineError::Coordinate(CoordinateError::LineOutOfBounds(Line::new(3)))
    );
}

#[test]
fn from_metadata_layer_drops_kind_and_ids_but_preserves_payload() {
    let mut layer = MetadataLayer::with_kind(
        MetadataLayerKind::custom("diagnostics"),
        BufferVersion::new(4),
    )
    .with_default_stickiness(Stickiness::Expand)
    .with_default_update_policy(TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion());
    layer.insert(range(0, 2), "alpha").unwrap();
    layer
        .insert_with_options(
            range(3, 5),
            Stickiness::Never,
            TrackedRangeUpdatePolicy::invalidate_when_collapsed(),
            "beta",
        )
        .unwrap();

    let set: VersionedRangeSet<&str> = layer.into();

    assert_eq!(set.version(), BufferVersion::new(4));
    assert_eq!(set.default_stickiness(), Stickiness::Expand);
    assert_eq!(
        set.default_update_policy(),
        TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion()
    );
    assert_eq!(set.len(), 2);

    let alpha = set.entry(0).unwrap();
    assert_eq!(alpha.range(), range(0, 2));
    assert_eq!(alpha.stickiness(), Stickiness::Expand);
    assert_eq!(
        alpha.update_policy(),
        TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion()
    );
    assert_eq!(alpha.payload(), &"alpha");

    let beta = set.entry(1).unwrap();
    assert_eq!(beta.range(), range(3, 5));
    assert_eq!(beta.stickiness(), Stickiness::Never);
    assert_eq!(
        beta.update_policy(),
        TrackedRangeUpdatePolicy::invalidate_when_collapsed()
    );
    assert_eq!(beta.payload(), &"beta");
}

#[test]
fn into_metadata_layer_assigns_fresh_ids_and_carries_kind() {
    let mut set =
        VersionedRangeSet::new(BufferVersion::new(2)).with_default_stickiness(Stickiness::Expand);
    set.insert(range(0, 2), "alpha");
    set.insert_with_options(
        range(3, 5),
        Stickiness::Never,
        TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion(),
        "beta",
    );

    let layer = set.into_metadata_layer(MetadataLayerKind::custom("diagnostics"));

    assert_eq!(layer.kind(), &MetadataLayerKind::custom("diagnostics"));
    assert_eq!(layer.version(), BufferVersion::new(2));
    assert_eq!(layer.default_stickiness(), Stickiness::Expand);
    assert_eq!(layer.len(), 2);

    let ids = layer
        .as_slice()
        .iter()
        .map(|range| range.id().get())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![0, 1]);

    let alpha = &layer.as_slice()[0];
    assert_eq!(alpha.range(), range(0, 2));
    assert_eq!(alpha.stickiness(), Stickiness::Expand);
    assert_eq!(alpha.metadata(), &"alpha");

    let beta = &layer.as_slice()[1];
    assert_eq!(beta.range(), range(3, 5));
    assert_eq!(beta.stickiness(), Stickiness::Never);
    assert_eq!(
        beta.update_policy(),
        TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion()
    );
    assert_eq!(beta.metadata(), &"beta");
}

#[test]
fn metadata_layer_round_trip_through_versioned_range_set_preserves_followability() {
    let mut buffer = buffer("abcdef");
    let mut layer = MetadataLayer::with_kind(MetadataLayerKind::custom("syntax"), buffer.version())
        .with_default_stickiness(Stickiness::Expand);
    layer.insert(range(1, 5), "match").unwrap();
    layer
        .insert_with_stickiness(range(5, 5), Stickiness::Never, "marker")
        .unwrap();

    // 通过 VersionedRangeSet 转一圈再回到 MetadataLayer，应保持每条 entry 的 stickiness。
    let set: VersionedRangeSet<&str> = layer.into();
    assert_eq!(set.entry(0).unwrap().stickiness(), Stickiness::Expand);
    assert_eq!(set.entry(1).unwrap().stickiness(), Stickiness::Never);

    let mut round_tripped = set.into_metadata_layer(MetadataLayerKind::custom("syntax"));
    assert_eq!(round_tripped.kind(), &MetadataLayerKind::custom("syntax"));
    assert_eq!(round_tripped.default_stickiness(), Stickiness::Expand);

    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );
    round_tripped.update_through_delta_event(&event).unwrap();

    assert_eq!(round_tripped.version(), buffer.version());
    // match: Stickiness::Expand → 起点吸附在插入前 (1)，终点吸附在插入后 (8)。
    assert_eq!(round_tripped.as_slice()[0].range(), range(1, 8));
    // marker: Stickiness::Never 的空 range，被插入推到 8。
    assert_eq!(round_tripped.as_slice()[1].range(), range(8, 8));
}
