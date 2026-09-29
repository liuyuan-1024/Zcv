use super::*;
use crate::{Buffer, BufferConfig, Edit, TransactionMetadata};

#[test]
fn snapshot_coordinates_define_empty_document_and_eof_boundaries() {
    let empty =
        Buffer::from_text(String::new(), BufferConfig::default()).expect("空文档快照应能创建");
    let empty_snapshot = empty.snapshot();
    assert_eq!(empty_snapshot.len_bytes(), ByteOffset::ZERO);
    assert_eq!(empty_snapshot.line_count(), 1);
    assert_eq!(
        empty_snapshot.line_start_byte(Line::ZERO).unwrap(),
        ByteOffset::ZERO
    );
    assert!(empty_snapshot.line_start_byte(Line::new(1)).is_err());
    assert_eq!(
        empty_snapshot.byte_to_line(ByteOffset::ZERO).unwrap(),
        Line::ZERO
    );

    let text =
        Buffer::from_text("a\n".to_owned(), BufferConfig::default()).expect("带换行文本应能创建");
    let snapshot = text.snapshot();
    assert_eq!(snapshot.line_count(), 2);
    assert_eq!(
        snapshot.line_start_byte(Line::new(1)).unwrap(),
        ByteOffset::new(2)
    );
    assert_eq!(
        snapshot.byte_to_line(snapshot.len_bytes()).unwrap(),
        Line::new(1)
    );
    assert!(snapshot.line_start_byte(Line::new(2)).is_err());
}

#[test]
fn snapshot_remains_immutable_when_buffer_advances() {
    let mut buffer =
        Buffer::from_text("a".to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let before = buffer.snapshot();

    buffer
        .edit(
            [Edit::insert(ByteOffset::new(1), "b").unwrap()],
            TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");
    let after = buffer.snapshot();

    assert_ne!(before.version(), after.version());
    assert_eq!(before.len_bytes(), ByteOffset::new(1));
    assert_eq!(after.len_bytes(), ByteOffset::new(2));
    assert_eq!(
        before
            .slice_byte_range(ByteOffset::ZERO, before.len_bytes())
            .unwrap()
            .as_str(),
        "a"
    );
    assert_eq!(
        after
            .slice_byte_range(ByteOffset::ZERO, after.len_bytes())
            .unwrap()
            .as_str(),
        "ab"
    );
}

#[test]
fn stable_anchor_order_includes_document_end() {
    let mut buffer = Buffer::from_text("中文 abc".to_owned(), BufferConfig::default()).unwrap();
    let before = buffer.snapshot();
    let interior = before.anchor_after(ByteOffset::new("中文 ".len()));
    let end = before.anchor_before(before.len_bytes());
    assert_eq!(before.stable_anchor_cmp(&interior, &end), Ordering::Less);
    buffer
        .edit(
            [Edit::insert(before.len_bytes(), " appended").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let after = buffer.snapshot();
    assert_eq!(after.stable_anchor_cmp(&interior, &end), Ordering::Less);
    assert_eq!(
        after.stable_anchor_cmp(&end, &after.anchor_before(after.len_bytes())),
        Ordering::Less
    );
    assert_eq!(end.resolve_in(&after).unwrap(), before.len_bytes());
}

#[test]
fn stable_anchor_order_preserves_empty_document_boundaries() {
    let mut buffer = Buffer::from_text(String::new(), BufferConfig::default()).unwrap();
    let empty = buffer.snapshot();
    let start = empty.anchor_before(ByteOffset::ZERO);
    let end = empty.anchor_after(ByteOffset::ZERO);
    buffer
        .edit(
            [Edit::insert(ByteOffset::ZERO, "one\ntwo\n").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let snapshot = buffer.snapshot();
    let middle = snapshot.anchor_after(ByteOffset::new(4));
    assert_eq!(snapshot.stable_anchor_cmp(&start, &middle), Ordering::Less);
    assert_eq!(snapshot.stable_anchor_cmp(&middle, &end), Ordering::Less);
    assert_eq!(start.resolve_in(&snapshot).unwrap(), ByteOffset::ZERO);
    assert_eq!(end.resolve_in(&snapshot).unwrap(), snapshot.len_bytes());
}

#[test]
fn stable_anchor_order_keeps_terminal_boundaries_after_prefix_and_suffix_insertions() {
    let mut buffer = Buffer::from_text("original\n".to_owned(), BufferConfig::default()).unwrap();
    let before = buffer.snapshot();
    let start = before.anchor_before(ByteOffset::ZERO);
    let end = before.anchor_after(before.len_bytes());
    buffer
        .edit(
            [
                Edit::insert(ByteOffset::ZERO, "prefix\n").unwrap(),
                Edit::insert(before.len_bytes(), "suffix\n").unwrap(),
            ],
            TransactionMetadata::default(),
        )
        .unwrap();
    let after = buffer.snapshot();
    for offset in [ByteOffset::new(1), ByteOffset::new(18)] {
        let inserted = after.anchor_after(offset);
        assert_eq!(after.stable_anchor_cmp(&start, &inserted), Ordering::Less);
        assert_eq!(after.stable_anchor_cmp(&inserted, &end), Ordering::Less);
    }
}
