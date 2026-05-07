//! M10 机器契约：锁定 MetadataRange 与 MetadataLayer 的外部区间承载语义。
//!
//! M10A 只验证泛型 metadata、版本绑定、范围跟随、失效策略和基础查询；
//! 不引入 diagnostics / highlight / breakpoint 的业务生成逻辑。

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, Edit, MetadataError, MetadataLayer,
    MetadataLayerKind, MetadataRange, MetadataRangeId, MetadataRangeUpdate, Stickiness, TextRange,
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

fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> zom_engine::DeltaEvent {
    let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
    buffer.apply_transaction(tx).unwrap();
    buffer.last_delta_event().unwrap().clone()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExternalPayload {
    source: &'static str,
    code: u32,
}

#[test]
fn metadata_range_binds_generic_payload_to_versioned_tracked_range() {
    let metadata_range = MetadataRange::new(
        MetadataRangeId::new(7),
        BufferVersion::INITIAL,
        range(1, 4),
        Stickiness::Expand,
        ExternalPayload {
            source: "external",
            code: 42,
        },
    );

    assert_eq!(metadata_range.id(), MetadataRangeId::new(7));
    assert_eq!(metadata_range.version(), BufferVersion::INITIAL);
    assert_eq!(metadata_range.range(), range(1, 4));
    assert_eq!(metadata_range.stickiness(), Stickiness::Expand);
    assert_eq!(
        metadata_range.metadata(),
        &ExternalPayload {
            source: "external",
            code: 42
        }
    );
}

#[test]
fn metadata_layer_inserts_ranges_with_stable_ids_and_layer_kind() {
    let mut layer = MetadataLayer::with_kind(
        MetadataLayerKind::custom("diagnostics-from-host"),
        BufferVersion::INITIAL,
    )
    .with_default_stickiness(Stickiness::Never);

    let first = layer.insert(range(1, 3), "warning").unwrap();
    let second = layer
        .insert_with_stickiness(range(3, 3), Stickiness::Expand, "bookmark")
        .unwrap();

    assert_eq!(
        layer.kind(),
        &MetadataLayerKind::custom("diagnostics-from-host")
    );
    assert_eq!(layer.version(), BufferVersion::INITIAL);
    assert_eq!(first, MetadataRangeId::INITIAL);
    assert_eq!(second, MetadataRangeId::new(1));
    assert_eq!(layer.len(), 2);
    assert_eq!(layer.get(first).unwrap().metadata(), &"warning");
    assert_eq!(layer.get(second).unwrap().stickiness(), Stickiness::Expand);
}

#[test]
fn metadata_layer_updates_ranges_through_delta_event() {
    let mut buffer = buffer("abcdef");
    let mut layer = MetadataLayer::with_kind(MetadataLayerKind::SearchMatch, buffer.version())
        .with_default_stickiness(Stickiness::Expand);
    let match_id = layer.insert(range(1, 5), "match").unwrap();
    let marker_id = layer
        .insert_with_stickiness(range(5, 5), Stickiness::Never, "marker")
        .unwrap();

    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );
    let updates = layer.update_through_delta_event(&event).unwrap();

    assert_eq!(layer.version(), buffer.version());
    assert_eq!(
        updates,
        vec![
            MetadataRangeUpdate::Mapped {
                id: match_id,
                range: range(1, 8),
                version: buffer.version(),
            },
            MetadataRangeUpdate::Mapped {
                id: marker_id,
                range: range(8, 8),
                version: buffer.version(),
            },
        ]
    );
    assert_eq!(layer.get(match_id).unwrap().range(), range(1, 8));
    assert_eq!(layer.get(marker_id).unwrap().range(), range(8, 8));
    assert_eq!(layer.get(match_id).unwrap().metadata(), &"match");
}

#[test]
fn metadata_layer_drops_invalidated_ranges_and_reports_last_mapped_range() {
    let mut buffer = buffer("abcdef");
    let mut layer = MetadataLayer::with_kind(MetadataLayerKind::Diagnostics, buffer.version());
    let policy = TrackedRangeUpdatePolicy::new(
        TrackedRangeInvalidationPolicy::WhenTouchedByDeletion,
        TrackedRangeCollapsePolicy::Keep,
    );
    let diagnostic_id = layer
        .insert_with_options(range(1, 5), Stickiness::Never, policy, "diagnostic")
        .unwrap();
    let bookmark_id = layer
        .insert_with_options(range(5, 5), Stickiness::Never, policy, "bookmark")
        .unwrap();

    let event = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);
    let updates = layer.update_through_delta_event(&event).unwrap();

    assert_eq!(
        updates,
        vec![
            MetadataRangeUpdate::Invalidated {
                id: diagnostic_id,
                range: range(1, 3),
                version: buffer.version(),
            },
            MetadataRangeUpdate::Mapped {
                id: bookmark_id,
                range: range(3, 3),
                version: buffer.version(),
            },
        ]
    );
    assert!(layer.get(diagnostic_id).is_none());
    assert_eq!(layer.get(bookmark_id).unwrap().range(), range(3, 3));
    assert_eq!(layer.len(), 1);
}

#[test]
fn metadata_layer_rejects_unrelated_delta_event_without_partial_mutation() {
    let mut buffer = buffer("abcdef");
    let mut layer = MetadataLayer::new(BufferVersion::new(99));
    let id = layer.insert(range(1, 3), "stale").unwrap();

    let event = apply(&mut buffer, vec![Edit::delete(range(1, 2))]);
    let err = layer.update_through_delta_event(&event).unwrap_err();

    assert_eq!(
        err,
        MetadataError::VersionMismatch {
            expected: BufferVersion::INITIAL,
            actual: BufferVersion::new(99),
        }
    );
    assert_eq!(layer.version(), BufferVersion::new(99));
    assert_eq!(layer.get(id).unwrap().range(), range(1, 3));
}

#[test]
fn multiple_metadata_layers_can_follow_the_same_delta_independently() {
    let mut buffer = buffer("abcdef");
    let mut diagnostics =
        MetadataLayer::with_kind(MetadataLayerKind::Diagnostics, buffer.version());
    let mut bookmarks = MetadataLayer::with_kind(MetadataLayerKind::Bookmark, buffer.version())
        .with_default_stickiness(Stickiness::Expand);
    let diagnostic_id = diagnostics.insert(range(1, 3), "diagnostic").unwrap();
    let bookmark_id = bookmarks.insert(range(3, 3), "bookmark").unwrap();

    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(3), "XYZ".to_string()).unwrap()],
    );

    diagnostics.update_through_delta_event(&event).unwrap();
    bookmarks.update_through_delta_event(&event).unwrap();

    assert_eq!(diagnostics.get(diagnostic_id).unwrap().range(), range(1, 3));
    assert_eq!(bookmarks.get(bookmark_id).unwrap().range(), range(3, 6));
    assert_eq!(diagnostics.kind(), &MetadataLayerKind::Diagnostics);
    assert_eq!(bookmarks.kind(), &MetadataLayerKind::Bookmark);
}

#[test]
fn metadata_layer_can_query_by_text_range_and_offset() {
    let mut layer = MetadataLayer::new(BufferVersion::INITIAL);
    let left = layer.insert(range(0, 2), "left").unwrap();
    let marker = layer
        .insert_with_stickiness(range(3, 3), Stickiness::Expand, "marker")
        .unwrap();
    let boundary_marker = layer
        .insert_with_stickiness(range(4, 4), Stickiness::Expand, "boundary-marker")
        .unwrap();
    let right = layer.insert(range(4, 6), "right").unwrap();

    let intersecting = layer
        .ranges_intersecting(range(1, 4))
        .map(|range| range.id())
        .collect::<Vec<_>>();
    let containing = layer
        .ranges_containing(c(3))
        .map(|range| range.id())
        .collect::<Vec<_>>();

    assert_eq!(intersecting, vec![left, marker]);
    assert_eq!(containing, vec![marker]);
    assert_eq!(
        layer
            .ranges_containing(c(4))
            .map(|range| range.id())
            .collect::<Vec<_>>(),
        vec![boundary_marker, right]
    );
}

#[test]
fn metadata_range_can_preview_its_update_without_mutating_payload() {
    let mut buffer = buffer("abcdef");
    let metadata_range = MetadataRange::with_policy(
        MetadataRangeId::new(3),
        buffer.version(),
        range(1, 5),
        Stickiness::Never,
        TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion(),
        "external-result",
    );
    let event = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);

    assert_eq!(
        metadata_range.map_through_delta_event(&event),
        Ok(TrackedRangeUpdate::Invalidated {
            range: range(1, 3),
            version: buffer.version(),
        })
    );
    assert_eq!(metadata_range.metadata(), &"external-result");
    assert_eq!(metadata_range.range(), range(1, 5));
}
