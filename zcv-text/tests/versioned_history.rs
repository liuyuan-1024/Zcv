//! 版本历史能力：历史可见性与锚点区间可见性。

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
fn has_edits_since_reports_a_real_reinsert_after_delete() {
    // 删除一段文本后把同样文本原位插回：旧 fragment 被删除、新 fragment 插入，属于有编辑。
    // 这里不是 undo 恢复，而是新插入，因此不共享原片段身份。
    let mut buffer = buffer("abc");
    let v0 = buffer.version();
    buffer
        .edit([Edit::delete(range(1, 2))], TransactionMetadata::default())
        .unwrap();
    buffer
        .edit(
            [Edit::insert(b(1), "b".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();

    assert_eq!(buffer_text(&buffer), "abc");
    assert!(buffer.snapshot().has_edits_since(v0).unwrap());
}

#[test]
fn has_edits_since_after_delete_then_undo_matches_zed() {
    // undo 回放按被回退的版本区间恢复原片段可见性，等价 undo map，
    // 因此「删除后用 undo 原位还原同一文本」判为无编辑。
    let mut buffer = buffer("abc");
    let v0 = buffer.version();
    buffer
        .edit([Edit::delete(range(1, 2))], TransactionMetadata::default())
        .unwrap();
    let v1 = buffer.version();
    buffer.undo().unwrap().expect("undo 应成功");

    assert_eq!(buffer_text(&buffer), "abc");
    // 相对删除前：片段可见性恢复原状，判为无编辑。
    assert!(
        !buffer.snapshot().has_edits_since(v0).unwrap(),
        "undo 恢复原片段可见性后应判为无编辑"
    );
    // 相对删除后：该片段当时不可见、现在可见，属于有编辑。可见性不能因 undo 被重写掉。
    assert!(
        buffer.snapshot().has_edits_since(v1).unwrap(),
        "删除后的历史版本上片段曾不可见，应判为有编辑"
    );
}

#[test]
fn has_edits_since_in_range_checks_only_the_requested_anchor_range() {
    let mut buffer = buffer("abcdef");
    let v0 = buffer.version();
    let before = buffer.snapshot();
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
    let anchors = |start, end| before.anchor_before(b(start))..before.anchor_before(b(end));
    assert!(
        snapshot
            .has_edits_since_in_range(v0, anchors(0, 1))
            .unwrap()
    );
    assert!(
        snapshot
            .has_edits_since_in_range(v0, anchors(4, 6))
            .unwrap()
    );
    assert!(
        !snapshot
            .has_edits_since_in_range(v0, anchors(1, 4))
            .unwrap()
    );
    assert!(
        !snapshot
            .has_edits_since_in_range(v0, anchors(1, 1))
            .unwrap()
    );
}

#[test]
fn anchor_range_visibility_survives_edit_log_eviction_and_detects_deleted_text() {
    let mut config = BufferConfig::default();
    config.large_file.max_edit_history_entries = 1;
    let mut buffer = Buffer::from_text("abcdef".into(), config).unwrap();
    let before = buffer.snapshot();
    let version = before.version();
    let changed = before.anchor_before(b(2))..before.anchor_before(b(4));
    let unchanged = before.anchor_before(b(0))..before.anchor_before(b(2));

    buffer
        .edit([Edit::delete(range(2, 4))], TransactionMetadata::default())
        .unwrap();
    for _ in 0..3 {
        let end = buffer.snapshot().len_bytes();
        buffer
            .edit(
                [Edit::insert(end, "x").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
    }

    let snapshot = buffer.snapshot();
    assert!(snapshot.edits_since(version).is_err());
    assert!(
        snapshot.has_edits_since_in_range(version, changed).unwrap(),
        "范围端点收敛到同一当前坐标后，仍应发现已删除的旧片段"
    );
    assert!(
        !snapshot
            .has_edits_since_in_range(version, unchanged)
            .unwrap()
    );
}

#[test]
fn anchor_range_respects_boundary_insertions_and_undo_visibility() {
    let mut buffer = buffer("abcdef");
    let before = buffer.snapshot();
    let version = before.version();
    let selected = before.anchor_before(b(2))..before.anchor_before(b(4));

    buffer
        .edit(
            [Edit::insert(b(4), "!").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    assert!(
        !buffer
            .snapshot()
            .has_edits_since_in_range(version, selected.clone())
            .unwrap()
    );

    buffer
        .edit(
            [Edit::insert(b(2), "?").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    assert!(
        buffer
            .snapshot()
            .has_edits_since_in_range(version, selected.clone())
            .unwrap()
    );

    buffer.undo().unwrap().expect("撤销区块起点插入");
    assert!(
        !buffer
            .snapshot()
            .has_edits_since_in_range(version, selected)
            .unwrap()
    );
}

#[test]
fn multi_edit_with_length_change_keeps_insertion_index_aligned_with_text() {
    // 一批 Edit 以旧文本坐标为基准；存储按旧坐标倒序应用。
    // 插入索引必须遵守同一排序契约，否则前一条 edit 改变长度后，后一条 edit 的旧坐标会在当前文本上指向错误片段，索引与文本分叉。
    let mut buffer = buffer("abcdef");
    let v0 = buffer.version();
    let before = buffer.snapshot();
    buffer
        .edit(
            [
                Edit::replace(range(0, 1), "XXXX".to_string()),
                Edit::delete(range(5, 6)),
            ],
            TransactionMetadata::default(),
        )
        .unwrap();
    assert_eq!(buffer_text(&buffer), "XXXXbcde");

    let snapshot = buffer.snapshot();
    let anchors = |start, end| before.anchor_before(b(start))..before.anchor_before(b(end));
    // 旧 'c'（offset 2..3）在存储中保留，应判为无编辑。
    assert!(
        !snapshot
            .has_edits_since_in_range(v0, anchors(2, 3))
            .unwrap(),
        "保留的旧 'c' 不应被判为有编辑"
    );
    // 旧 'f'（offset 5..6）在存储中已删除，应判为有编辑。
    assert!(
        snapshot
            .has_edits_since_in_range(v0, anchors(5, 6))
            .unwrap(),
        "已删除的旧 'f' 应被判为有编辑"
    );
}
