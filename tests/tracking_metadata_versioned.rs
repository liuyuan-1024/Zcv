use zom_engine::*;

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn line(value: usize) -> Line {
    Line::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

fn line_range(start: usize, end: usize) -> LineRange {
    LineRange::new(line(start), line(end)).unwrap()
}

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn event_after(buffer: &mut Buffer, edit: Edit) -> DeltaEvent {
    buffer
        .apply_transaction(Transaction::from_edits(buffer.version(), vec![edit]).unwrap())
        .unwrap();
    buffer.last_delta_event().unwrap().clone()
}

#[test]
fn anchor_and_mark_should_map_through_delta_with_affinity_and_deleted_policy() {
    let mut buffer = buffer("abcd");
    let anchor = Anchor::new(buffer.version(), b(2)).with_affinity(Affinity::Before);
    let mark = Mark::new(b(2)).with_affinity(Affinity::After);
    let event = event_after(&mut buffer, Edit::insert(b(2), "XX".to_string()).unwrap());

    assert_eq!(
        anchor
            .map_through_delta_event(&event)
            .unwrap()
            .value()
            .offset(),
        b(2)
    );
    assert_eq!(
        mark.map_through_position_map(event.position_map())
            .value()
            .offset(),
        b(4)
    );

    let deleted = Anchor::new(event.new_version(), b(2));
    let delete_event = event_after(&mut buffer, Edit::delete(range(1, 4)));
    assert!(matches!(
        deleted
            .map_through_delta_event_with_deleted_policy(
                &delete_event,
                AnchorDeletedPolicy::Invalidate
            )
            .unwrap(),
        AnchorUpdate::Invalidated { mark, version }
            if mark.offset() == b(1) && version == delete_event.new_version()
    ));
}

#[test]
fn tracked_range_should_reject_mismatched_versions_and_invalidate_when_policy_matches_deletion() {
    let start = Anchor::new(BufferVersion::INITIAL, b(3));
    let end = Anchor::new(BufferVersion::new(1), b(6));
    let err = TrackedRange::new(start, end, Stickiness::Never).unwrap_err();
    assert!(matches!(
        err,
        EngineError::Anchor(AnchorError::RangeVersionMismatch { .. })
    ));

    let mut buffer = buffer("abcdef");
    let tracked = TrackedRange::from_range(buffer.version(), range(1, 4), Stickiness::Never);
    let event = event_after(&mut buffer, Edit::delete(range(1, 4)));
    let update = tracked
        .map_through_delta_event_with_policy(
            &event,
            TrackedRangeUpdatePolicy::invalidate_when_fully_deleted(),
        )
        .unwrap();

    assert!(matches!(
        update,
        TrackedRangeUpdate::Invalidated { range, version }
            if range == TextRange::new(b(1), b(1)).unwrap() && version == event.new_version()
    ));
}

#[test]
fn metadata_layer_should_insert_query_update_and_drop_invalidated_ranges() {
    let mut buffer = buffer("one\ntwo\nthree");
    let mut layer = MetadataLayer::with_kind(MetadataLayerKind::custom("notes"), buffer.version())
        .with_default_update_policy(TrackedRangeUpdatePolicy::invalidate_when_fully_deleted());
    let first = layer.insert(range(0, 3), "one").unwrap();
    let second = layer.insert(range(4, 7), "two").unwrap();

    assert_eq!(layer.len(), 2);
    assert_eq!(layer.get(first).unwrap().metadata(), &"one");
    assert_eq!(
        layer
            .ranges_intersecting(range(2, 5))
            .map(|entry| *entry.metadata())
            .collect::<Vec<_>>(),
        vec!["one", "two"]
    );
    assert_eq!(
        layer
            .ranges_in_line_range(&buffer, line_range(1, 2))
            .unwrap()
            .len(),
        1
    );

    let event = event_after(&mut buffer, Edit::delete(range(4, 7)));
    let updates = layer.update_through_delta_event(&event).unwrap();

    assert_eq!(updates.len(), 2);
    assert_eq!(layer.version(), event.new_version());
    assert!(layer.get(second).is_none());
    assert_eq!(layer.len(), 1);
}

#[test]
fn metadata_layers_should_replace_query_by_kind_and_discard_stale_layers() {
    let buffer = buffer("abc\ndef");
    let mut layers = MetadataLayers::new();
    let kind = MetadataLayerKind::custom("analysis");

    layers
        .replace_layer_ranges(
            kind.clone(),
            buffer.version(),
            vec![(range(0, 3), "alpha"), (range(4, 7), "beta")],
        )
        .unwrap();

    assert_eq!(layers.len(), 1);
    assert_eq!(
        layers
            .ranges_for_kind_intersecting(&kind, range(1, 5))
            .map(|entry| *entry.metadata())
            .collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );
    assert_eq!(
        layers
            .ranges_for_kind_in_line_range(&kind, &buffer, line_range(1, 2))
            .unwrap()
            .len(),
        1
    );

    let stale = layers.discard_stale(BufferVersion::new(99));
    assert_eq!(stale.len(), 1);
    assert!(layers.is_empty());
}

#[test]
fn versioned_result_should_reject_stale_delta_and_remap_payload_through_position_map() {
    let mut buffer = buffer("abcdef");
    let result = VersionedResult::new(buffer.version(), range(2, 5));
    let event = event_after(&mut buffer, Edit::insert(b(0), "XX".to_string()).unwrap());

    let remapped = result
        .try_remap(&event, |range, map| Ok(map.map_old_range(range).value()))
        .unwrap();

    assert_eq!(remapped.version(), event.new_version());
    assert_eq!(*remapped.value(), range(4, 7));

    let stale = VersionedResult::new(BufferVersion::new(42), range(0, 1));
    let err = stale
        .try_remap(&event, |range, map| Ok(map.map_old_range(range).value()))
        .unwrap_err();
    assert!(matches!(
        err,
        EngineError::Versioned(VersionedResultError::VersionMismatch { .. })
    ));
}

#[test]
fn versioned_range_set_should_update_query_export_utf16_and_roundtrip_metadata_layer() {
    let mut buffer = buffer("a😀b\ncd");
    let snapshot = buffer.snapshot();
    let mut set = VersionedRangeSet::new(buffer.version());

    let idx = set
        .try_insert_utf16_range(
            &snapshot,
            Utf16Position::new(line(0), Utf16Offset::new(1)),
            Utf16Position::new(line(0), Utf16Offset::new(3)),
            "emoji",
        )
        .unwrap();
    set.insert(range(7, 9), "cd");

    assert_eq!(idx, 0);
    assert_eq!(set.entries_containing(b(1)).count(), 1);
    assert_eq!(
        set.entries_in_line_range(&buffer, line_range(1, 2))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        set.try_export_entries_to_utf16(&snapshot).unwrap()[0],
        (
            Utf16Position::new(line(0), Utf16Offset::new(1)),
            Utf16Position::new(line(0), Utf16Offset::new(3)),
            &"emoji"
        )
    );

    let event = event_after(&mut buffer, Edit::insert(b(0), "Z".to_string()).unwrap());
    let updates = set.update_through_delta_event(&event).unwrap();
    assert_eq!(updates.len(), 2);
    assert_eq!(set.version(), event.new_version());
    assert_eq!(set.entry(0).unwrap().range(), range(2, 6));

    let layer = set.into_metadata_layer(MetadataLayerKind::custom("roundtrip"));
    let roundtrip: VersionedRangeSet<&str> = VersionedRangeSet::from(layer);
    assert_eq!(roundtrip.len(), 2);
    assert_eq!(roundtrip.entry(0).unwrap().payload(), &"emoji");
}
