use zcv_text::*;

#[path = "common/buffer.rs"]
mod buffer;
#[path = "common/byte_range.rs"]
mod byte_range;

use buffer::buffer;
use byte_range::{b, range};

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
    config.large_file.max_edit_history_entries = 1;
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
fn coordinate_edits_since_survives_edit_log_eviction() {
    let mut config = BufferConfig::default();
    config.large_file.max_edit_history_entries = 1;
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
    let current = buffer.version();

    // 带文本编辑日志已被裁剪，edits_since 显式失败。
    assert!(buffer.snapshot().edits_since(v0).is_err());

    // 坐标索引不衰减：仍能给出 v0 → current 的坐标编辑。
    let batch = buffer
        .snapshot()
        .coordinate_edits_since(v0)
        .expect("坐标索引应覆盖被裁剪的版本");
    assert_eq!(batch.old_version(), Some(v0));
    assert_eq!(batch.new_version(), Some(current));
    assert!(!batch.patch().edits().is_empty());
}

#[test]
fn anchor_still_resolves_after_the_text_edit_log_is_evicted() {
    // 分别以条目数与字节预算逼出带文本 EditLog 的 eviction；坐标索引不衰减。
    for (entries, bytes) in [(1usize, 0usize), (usize::MAX, 1usize)] {
        let mut config = BufferConfig::default();
        config.large_file.max_edit_history_entries = entries;
        config.large_file.max_edit_history_bytes = bytes;
        let mut buffer = Buffer::from_text("abc".to_string(), config).unwrap();
        let anchor = buffer.snapshot().anchor_before(b(3));
        let anchor_version = anchor.version();

        for _ in 0..4 {
            let end = buffer.len_bytes();
            buffer
                .edit(
                    [Edit::insert(end, "x".to_string()).unwrap()],
                    TransactionMetadata::default(),
                )
                .unwrap();
        }

        assert!(matches!(
            buffer.snapshot().edits_since(anchor_version),
            Err(TextError::VersionEvicted { .. })
        ));
        assert_eq!(anchor.resolve_in(&buffer.snapshot()).unwrap(), b(3));
    }
}

#[test]
fn replace_text_maps_old_anchors_through_the_same_coordinate_chain() {
    let mut buffer = buffer("abc");
    let anchor = buffer.snapshot().anchor_before(b(2));

    buffer.replace_text("XYZ abc".to_string()).unwrap();

    assert_eq!(anchor.resolve_in(&buffer.snapshot()).unwrap(), b(6));
}

#[test]
fn replace_text_preserves_anchor_affinity_at_an_actual_insertion() {
    let mut buffer = buffer("abc xyz");
    let anchor = buffer.snapshot().anchor_after(b(4));

    buffer.replace_text("abc NEW xyz".to_string()).unwrap();

    let snapshot = buffer.snapshot();
    assert_eq!(anchor.resolve_in(&snapshot).unwrap(), b(8));
}

#[test]
fn replace_text_maps_anchor_inside_replaced_token_to_the_replacement_start() {
    let mut buffer = buffer("abc");
    let before = buffer.snapshot().anchor_before(b(2));
    let after = buffer.snapshot().anchor_after(b(2));

    buffer.replace_text("XYZabc".to_string()).unwrap();

    let snapshot = buffer.snapshot();
    assert_eq!(before.resolve_in(&snapshot).unwrap(), b(0));
    assert_eq!(after.resolve_in(&snapshot).unwrap(), b(0));
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
    assert_eq!(before.resolve_in(&snapshot).unwrap(), b(1));
    assert_eq!(after.resolve_in(&snapshot).unwrap(), b(3));
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
    assert!(matches!(
        newer_anchor.resolve_in(&older),
        Err(TextError::Anchor(AnchorError::TargetBeforeSource { .. }))
    ));
}
