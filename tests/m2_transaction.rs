//! M2：编辑事务、Delta 与 ChangeSet
//!
//! 这组测试属于 `tests/` 机器契约测试：
//! - 只通过 public API 使用 `zom-engine`
//! - 不依赖 GPUI / 窗口系统
//! - 锁定 M2 阶段的事务语义、版本契约、失败原子性与 PositionMap 生成
//! - M3.5 起，Transaction / Edit / ChangeSet 均使用 CharOffset 坐标

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, CoordinateError, Edit, EditError, EditList,
    EngineError, Line, TextRange, Transaction, TransactionError,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(CharOffset::new(start), CharOffset::new(end)).unwrap()
}

fn tx_at(version: BufferVersion, edits: Vec<Edit>) -> Transaction {
    let edit_list = EditList::new(edits).unwrap();
    Transaction::new(version, edit_list).unwrap()
}

fn tx_for(buffer: &Buffer, edits: Vec<Edit>) -> Transaction {
    tx_at(buffer.version(), edits)
}

#[test]
fn edit_list_sorts_edits_by_start_char_offset() {
    let later = Edit::replace(range(5, 7), "B".to_string());
    let earlier = Edit::replace(range(0, 2), "A".to_string());

    let edits = EditList::new(vec![later, earlier]).unwrap();

    assert_eq!(edits.as_slice().len(), 2);
    assert_eq!(edits.as_slice()[0].range.start(), CharOffset::new(0));
    assert_eq!(edits.as_slice()[1].range.start(), CharOffset::new(5));
}

#[test]
fn edit_list_rejects_overlapping_edits() {
    let first = Edit::replace(range(0, 2), "A".to_string());
    let overlapping = Edit::replace(range(1, 3), "B".to_string());

    let err = EditList::new(vec![first, overlapping]).unwrap_err();

    assert!(matches!(err, EditError::OverlappingEdits { .. }));
}

#[test]
fn edit_list_allows_adjacent_edits() {
    let first = Edit::replace(range(0, 2), "A".to_string());
    let second = Edit::replace(range(2, 4), "B".to_string());

    let edits = EditList::new(vec![second, first]).unwrap();

    assert_eq!(edits.as_slice().len(), 2);
    assert_eq!(edits.as_slice()[0].range, range(0, 2));
    assert_eq!(edits.as_slice()[1].range, range(2, 4));
}

#[test]
fn transaction_rejects_empty_edit_list() {
    let edits = EditList::new(Vec::new()).unwrap();

    let err = Transaction::new(BufferVersion::INITIAL, edits).unwrap_err();

    assert!(matches!(err, TransactionError::EmptyTransaction));
}

#[test]
fn transaction_applies_single_insert_and_returns_delta() {
    let mut buffer = buffer("hello");
    let old_version = buffer.version();

    let edit = Edit::insert(CharOffset::new(5), " world".to_string()).unwrap();
    let tx = tx_for(&buffer, vec![edit]);

    let (delta, changeset) = buffer.apply_transaction(tx).unwrap();

    assert_eq!(buffer.text(), "hello world");
    assert_eq!(delta.old_version, old_version);
    assert_eq!(delta.new_version, buffer.version());
    assert_eq!(delta.edits.as_slice().len(), 1);
    assert_eq!(changeset.changed_ranges(), vec![range(5, 11)]);
    assert!(buffer.version() > old_version);
    assert!(buffer.is_dirty());
}

#[test]
fn transaction_applies_multibyte_insert_using_char_offsets() {
    let mut buffer = buffer("你a");

    let edit = Edit::insert(CharOffset::new(1), "好".to_string()).unwrap();
    let tx = tx_for(&buffer, vec![edit]);

    let (_, changeset) = buffer.apply_transaction(tx).unwrap();

    assert_eq!(buffer.text(), "你好a");
    assert_eq!(changeset.changed_ranges(), vec![range(1, 2)]);
}

#[test]
fn transaction_applies_multiple_edits_in_old_char_coordinate_space() {
    let mut buffer = buffer("Hello World");

    let replace_world = Edit::replace(range(6, 11), "Rust".to_string());
    let append_bang = Edit::insert(CharOffset::new(11), "!".to_string()).unwrap();
    let tx = tx_for(&buffer, vec![append_bang, replace_world]);

    let (delta, changeset) = buffer.apply_transaction(tx).unwrap();

    assert_eq!(buffer.text(), "Hello Rust!");
    assert_eq!(delta.edits.as_slice().len(), 2);

    // 旧文本末尾 11 被映射到新文本末尾 11。
    let position_map = changeset.position_map();
    assert_eq!(
        position_map.map_old_position(CharOffset::new(11)).value(),
        CharOffset::new(11)
    );
}

#[test]
fn transaction_rejects_stale_base_version() {
    let mut buffer = buffer("hello");

    buffer.insert(CharOffset::new(5), "!").unwrap();
    let current_text = buffer.text().to_string();
    let current_version = buffer.version();

    let stale_tx = tx_at(
        BufferVersion::INITIAL,
        vec![Edit::insert(CharOffset::new(0), "X".to_string()).unwrap()],
    );

    let err = buffer.apply_transaction(stale_tx).unwrap_err();

    assert!(matches!(
        err,
        EngineError::Transaction(TransactionError::VersionMismatch { .. })
    ));
    assert_eq!(buffer.text(), current_text);
    assert_eq!(buffer.version(), current_version);
}

#[test]
fn failed_transaction_does_not_change_text_version_dirty_or_line_index() {
    let mut buffer = buffer("hello\nworld");
    buffer.mark_saved();

    let before_text = buffer.text().to_string();
    let before_version = buffer.version();
    let before_dirty = buffer.is_dirty();
    let before_line_count = buffer.line_count();
    let before_second_line = buffer.line_start(Line::new(1)).unwrap();

    let valid_edit = Edit::insert(CharOffset::new(0), "prefix ".to_string()).unwrap();
    let invalid_edit = Edit::insert(CharOffset::new(999), "!".to_string()).unwrap();
    let tx = tx_for(&buffer, vec![valid_edit, invalid_edit]);

    let err = buffer.apply_transaction(tx).unwrap_err();

    assert!(matches!(
        err,
        EngineError::Edit(EditError::RangeOutOfBounds { .. })
            | EngineError::Coordinate(CoordinateError::OutOfBounds(_))
    ));
    assert_eq!(buffer.text(), before_text);
    assert_eq!(buffer.version(), before_version);
    assert_eq!(buffer.is_dirty(), before_dirty);
    assert_eq!(buffer.line_count(), before_line_count);
    assert_eq!(buffer.line_start(Line::new(1)).unwrap(), before_second_line);
}

#[test]
fn failed_transaction_on_crlf_middle_boundary_is_atomic() {
    let mut buffer = buffer("a\r\nb");
    let before_text = buffer.text().to_string();
    let before_version = buffer.version();

    let valid_edit = Edit::insert(buffer.len_chars(), "!".to_string()).unwrap();
    let invalid_crlf_boundary = Edit::insert(CharOffset::new(2), "x".to_string()).unwrap();
    let tx = tx_for(&buffer, vec![valid_edit, invalid_crlf_boundary]);

    let err = buffer.apply_transaction(tx).unwrap_err();

    assert!(matches!(
        err,
        EngineError::Edit(EditError::InvalidBoundary { offset })
            if offset == CharOffset::new(2)
    ));
    assert_eq!(buffer.text(), before_text);
    assert_eq!(buffer.version(), before_version);
}

#[test]
fn changeset_produces_position_map_after_delete() {
    let mut buffer = buffer("12345");

    let tx = tx_for(&buffer, vec![Edit::delete(range(2, 4))]);
    let (_, changeset) = buffer.apply_transaction(tx).unwrap();
    let position_map = changeset.position_map();

    assert_eq!(buffer.text(), "125");
    assert_eq!(
        position_map.map_old_position(CharOffset::new(0)).value(),
        CharOffset::new(0)
    );
    assert_eq!(
        position_map.map_old_position(CharOffset::new(1)).value(),
        CharOffset::new(1)
    );
    assert_eq!(
        position_map.map_old_position(CharOffset::new(2)).value(),
        CharOffset::new(2)
    );
    assert_eq!(
        position_map.map_old_position(CharOffset::new(3)).value(),
        CharOffset::new(2)
    );
    assert_eq!(
        position_map.map_old_position(CharOffset::new(4)).value(),
        CharOffset::new(2)
    );
    assert_eq!(
        position_map.map_old_position(CharOffset::new(5)).value(),
        CharOffset::new(3)
    );
}

#[test]
fn changeset_produces_position_map_after_insert() {
    let mut buffer = buffer("ab");

    let tx = tx_for(
        &buffer,
        vec![Edit::insert(CharOffset::new(1), "XYZ".to_string()).unwrap()],
    );
    let (_, changeset) = buffer.apply_transaction(tx).unwrap();
    let position_map = changeset.position_map();

    assert_eq!(buffer.text(), "aXYZb");
    assert_eq!(
        position_map.map_old_position(CharOffset::new(0)).value(),
        CharOffset::new(0)
    );
    assert_eq!(
        position_map.map_old_position(CharOffset::new(1)).value(),
        CharOffset::new(4)
    );
    assert_eq!(
        position_map.map_old_position(CharOffset::new(2)).value(),
        CharOffset::new(5)
    );
}

#[test]
fn changeset_position_map_maps_range_without_unchecked_constructor() {
    let mut buffer = buffer("abcdef");

    let tx = tx_for(&buffer, vec![Edit::replace(range(1, 3), "XYZ".to_string())]);
    let (_, changeset) = buffer.apply_transaction(tx).unwrap();

    let mapped = changeset.position_map().map_old_range(range(0, 6)).value();

    assert_eq!(buffer.text(), "aXYZdef");
    assert_eq!(mapped, range(0, 7));
}

#[test]
fn changed_ranges_reports_new_text_ranges() {
    let mut buffer = buffer("hello world");

    let shrink_hello = Edit::replace(range(0, 5), "hi".to_string());
    let delete_world = Edit::delete(range(6, 11));
    let tx = tx_for(&buffer, vec![shrink_hello, delete_world]);

    let (_, changeset) = buffer.apply_transaction(tx).unwrap();
    let ranges = changeset.changed_ranges();

    assert_eq!(buffer.text(), "hi ");
    assert_eq!(ranges, vec![range(0, 2), range(3, 3)]);
}

#[test]
fn changed_ranges_keeps_separate_non_adjacent_insertions() {
    let mut buffer = buffer("a b c");

    let insert_after_a = Edit::insert(CharOffset::new(1), "_1".to_string()).unwrap();
    let insert_after_b = Edit::insert(CharOffset::new(3), "_2".to_string()).unwrap();
    let tx = tx_for(&buffer, vec![insert_after_b, insert_after_a]);

    let (_, changeset) = buffer.apply_transaction(tx).unwrap();
    let ranges = changeset.changed_ranges();

    assert_eq!(buffer.text(), "a_1 b_2 c");
    assert_eq!(ranges, vec![range(1, 3), range(5, 7)]);
}
