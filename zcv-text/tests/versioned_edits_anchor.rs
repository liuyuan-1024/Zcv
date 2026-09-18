use zcv_text::*;
mod common;
use common::*;

#[test]
fn edits_since_composes_continuous_versions_into_old_and_new_coordinates() {
    let mut buffer = buffer("abc");
    let v0 = buffer.version();
    buffer
        .edit(
            [Edit::insert(b(3), "d".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let v1 = buffer.version();
    buffer
        .edit(
            [Edit::replace(range(0, 1), "X".to_string())],
            TransactionMetadata::default(),
        )
        .unwrap();
    let current = buffer.version();

    let snapshot = buffer.snapshot();
    let batch = snapshot.edits_since(v0).unwrap();
    assert_eq!(batch.old_version(), Some(v0));
    assert_eq!(batch.new_version(), Some(current));
    assert_eq!(
        batch
            .patch()
            .edits()
            .iter()
            .map(|edit| edit.old_range())
            .collect::<Vec<_>>(),
        vec![range(0, 1), range(3, 3)],
    );
    assert_eq!(
        batch
            .patch()
            .edits()
            .iter()
            .map(|edit| edit.new_range())
            .collect::<Vec<_>>(),
        vec![range(0, 1), range(3, 4)],
    );

    let since_v1 = snapshot.edits_since(v1).unwrap();
    assert_eq!(since_v1.patch().edits().len(), 1);
    assert_eq!(since_v1.patch().edits()[0].old_range(), range(0, 1));
    assert_eq!(since_v1.patch().edits()[0].new_range(), range(0, 1));
}

#[test]
fn edits_since_reports_eviction_when_the_version_left_the_log() {
    let mut config = BufferConfig::default();
    config.large_file.max_undo_history = 1;
    let mut buffer = Buffer::from_text("abc".to_string(), config).unwrap();
    let v0 = buffer.version();
    buffer
        .edit(
            [Edit::insert(b(3), "d".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    buffer
        .edit(
            [Edit::insert(b(4), "e".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();

    let error = buffer.snapshot().edits_since(v0).unwrap_err();
    assert!(matches!(error, TextError::VersionEvicted { requested, .. } if requested == v0));
}

#[test]
fn edits_since_in_range_keeps_only_overlapping_edits() {
    let mut buffer = buffer("abcdef");
    let base = buffer.version();
    buffer
        .edit(
            [
                Edit::replace(range(0, 1), "X".to_string()),
                Edit::replace(range(4, 5), "Y".to_string()),
            ],
            TransactionMetadata::default(),
        )
        .unwrap();

    let batch = buffer
        .snapshot()
        .edits_since_in_range(base, range(4, 6))
        .unwrap();
    assert_eq!(batch.patch().edits().len(), 1);
    assert_eq!(batch.patch().edits()[0].old_range(), range(4, 5));
}

#[test]
fn anchor_resolves_across_versions_with_affinity() {
    let mut buffer = buffer("abc");
    let (before, after) = {
        let snapshot = buffer.snapshot();
        (snapshot.anchor_before(b(1)), snapshot.anchor_after(b(1)))
    };
    buffer
        .edit(
            [Edit::insert(b(1), "XY".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();

    let snapshot = buffer.snapshot();
    assert_eq!(before.resolve_in(&snapshot), Some(b(1)));
    assert_eq!(after.resolve_in(&snapshot), Some(b(3)));
}

#[test]
fn anchor_does_not_resolve_into_an_older_snapshot() {
    let mut buffer = buffer("abc");
    let older = buffer.snapshot();
    buffer
        .edit(
            [Edit::insert(b(3), "d".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let newer_anchor = buffer.snapshot().anchor_after(b(0));
    assert!(newer_anchor.resolve_in(&older).is_none());
}
