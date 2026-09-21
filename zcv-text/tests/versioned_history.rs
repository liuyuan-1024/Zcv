//! 版本历史能力：历史可见性、当前坐标到旧版本的映射与按版本重建文本。

use zcv_text::*;

#[path = "common/buffer.rs"]
mod buffer;
#[path = "common/byte_range.rs"]
mod byte_range;
#[path = "common/full_text.rs"]
mod full_text;

use buffer::buffer;
use byte_range::{b, range};
use full_text::buffer_text;

#[test]
fn has_edits_since_tracks_net_edits_between_versions() {
    let mut buffer = buffer("abc");
    let v0 = buffer.version();
    assert!(
        !buffer.snapshot().has_edits_since(v0).unwrap(),
        "同版本没有编辑"
    );

    buffer
        .edit(
            [Edit::insert(b(3), "d".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let v1 = buffer.version();
    assert!(buffer.snapshot().has_edits_since(v0).unwrap());
    assert!(!buffer.snapshot().has_edits_since(v1).unwrap());

    // 插入后删除：净编辑相互抵消，从 v0 看没有编辑。
    buffer
        .edit([Edit::delete(range(3, 4))], TransactionMetadata::default())
        .unwrap();
    let v2 = buffer.version();
    assert!(
        !buffer.snapshot().has_edits_since(v0).unwrap(),
        "抵消后的净编辑为空"
    );
    assert!(buffer.snapshot().has_edits_since(v1).unwrap());
    assert!(!buffer.snapshot().has_edits_since(v2).unwrap());
}

#[test]
fn has_edits_since_in_range_checks_only_the_requested_old_range() {
    let mut buffer = buffer("abcdef");
    let v0 = buffer.version();
    buffer
        .edit(
            [
                Edit::replace(range(0, 1), "X".to_string()),
                Edit::replace(range(4, 5), "Y".to_string()),
            ],
            TransactionMetadata::default(),
        )
        .unwrap();

    let snapshot = buffer.snapshot();
    assert!(snapshot.has_edits_since_in_range(v0, range(0, 1)).unwrap());
    assert!(snapshot.has_edits_since_in_range(v0, range(4, 6)).unwrap());
    assert!(!snapshot.has_edits_since_in_range(v0, range(1, 4)).unwrap());
    assert!(!snapshot.has_edits_since_in_range(v0, range(1, 1)).unwrap());
}

#[test]
fn offsets_to_version_round_trips_with_edits_since() {
    let mut buffer = buffer("abcdef");
    let v0 = buffer.version();
    buffer
        .edit(
            [Edit::replace(range(1, 3), "XYZ".to_string())],
            TransactionMetadata::default(),
        )
        .unwrap();
    let snapshot = buffer.snapshot();

    let batch = snapshot.edits_since(v0).unwrap();
    let forward = batch.position_map();

    // 替换区外的坐标经旧→新、新→旧往返后保持不变。
    for offset in [0usize, 3, 4, 5, 6] {
        let old = b(offset);
        let new = forward.map_old_position(old).value();
        assert_eq!(
            snapshot.offsets_to_version([new], v0).unwrap(),
            vec![old],
            "offset {offset} 应往返一致"
        );
    }

    // 替换产生的新内容按 overshoot 收敛回旧区间内部。
    assert_eq!(snapshot.offsets_to_version([b(2)], v0).unwrap(), vec![b(2)]);
    assert_eq!(snapshot.offsets_to_version([b(3)], v0).unwrap(), vec![b(3)]);
}

#[test]
fn range_to_version_maps_a_current_range_back_to_the_old_version() {
    let mut buffer = buffer("abcdef");
    let v0 = buffer.version();
    buffer
        .edit(
            [Edit::replace(range(1, 3), "XYZ".to_string())],
            TransactionMetadata::default(),
        )
        .unwrap();
    let snapshot = buffer.snapshot();

    assert_eq!(
        snapshot.range_to_version(range(4, 6), v0).unwrap(),
        range(3, 5)
    );
    assert_eq!(
        snapshot.range_to_version(range(0, 1), v0).unwrap(),
        range(0, 1)
    );
    assert_eq!(
        snapshot.range_to_version(range(1, 4), v0).unwrap(),
        range(1, 3)
    );
}

#[test]
fn text_for_version_rebuilds_each_historical_version() {
    let mut buffer = buffer("abcdef");
    let v0 = buffer.version();
    buffer
        .edit(
            [Edit::replace(range(1, 3), "XYZ".to_string())],
            TransactionMetadata::default(),
        )
        .unwrap();
    let v1 = buffer.version();
    buffer
        .edit(
            [Edit::insert(b(7), "!".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let v2 = buffer.version();

    let snapshot = buffer.snapshot();
    assert_eq!(snapshot.text_for_version(v2).unwrap(), "aXYZdef!");
    assert_eq!(snapshot.text_for_version(v1).unwrap(), "aXYZdef");
    assert_eq!(snapshot.text_for_version(v0).unwrap(), "abcdef");
    assert_eq!(buffer_text(&snapshot), "aXYZdef!");
}

#[test]
fn text_for_version_stays_available_after_undo_and_redo() {
    let mut buffer = buffer("abc");
    let v0 = buffer.version();
    buffer
        .edit(
            [Edit::insert(b(3), "d".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let v1 = buffer.version();

    buffer.undo().unwrap().expect("undo 应成功");
    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(
        buffer.snapshot().text_for_version(v0).unwrap(),
        "abc",
        "undo 后必须能重建历史文本，而不是 HistoryTextUnavailable"
    );
    assert_eq!(buffer.snapshot().text_for_version(v1).unwrap(), "abcd");

    buffer.redo().unwrap().expect("redo 应成功");
    assert_eq!(buffer_text(&buffer), "abcd");
    assert_eq!(buffer.snapshot().text_for_version(v0).unwrap(), "abc");
    assert_eq!(buffer.snapshot().text_for_version(v1).unwrap(), "abcd");
}

#[test]
fn text_for_version_rejects_versions_newer_than_the_snapshot() {
    let buffer = buffer("abc");
    let future = buffer.version().next().unwrap();
    assert!(matches!(
        buffer.snapshot().text_for_version(future),
        Err(TextError::Anchor(AnchorError::TargetBeforeSource { .. }))
    ));
}

#[test]
fn text_for_version_reports_eviction_after_the_edit_log_is_trimmed() {
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

    assert!(matches!(
        buffer.snapshot().text_for_version(v0),
        Err(TextError::VersionEvicted { requested, .. }) if requested == v0
    ));
}

#[test]
fn text_for_version_reports_unavailable_history_when_inverse_edits_were_not_kept() {
    let mut buffer = buffer("abc");
    let v0 = buffer.version();
    buffer
        .edit(
            [Edit::insert(b(3), "d".to_string()).unwrap()],
            TransactionMetadata::new(TransactionSource::Programmatic).without_history(),
        )
        .unwrap();

    assert!(matches!(
        buffer.snapshot().text_for_version(v0),
        Err(TextError::HistoryTextUnavailable { requested, .. }) if requested == v0
    ));
}
