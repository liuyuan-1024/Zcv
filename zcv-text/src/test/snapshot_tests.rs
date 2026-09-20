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
