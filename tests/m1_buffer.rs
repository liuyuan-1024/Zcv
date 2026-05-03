use zom_engine::{
    Buffer, BufferConfig, ByteOffset, CoordinateError, EditError, EngineError, Line, LogicalColumn,
    Position, TextRange,
};

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap()
}

fn assert_buffer_state(buffer: &Buffer, text: &str, version: u64, dirty: bool, line_count: usize) {
    assert_eq!(buffer.text(), text);
    assert_eq!(buffer.version().get(), version);
    assert_eq!(buffer.is_dirty(), dirty);
    assert_eq!(buffer.line_count(), line_count);
}

#[test]
fn buffer_can_be_created_and_read() {
    let buffer = Buffer::from_text("hello".to_string(), BufferConfig::default()).unwrap();

    assert_eq!(buffer.text(), "hello");
    assert_eq!(buffer.len_bytes().get(), 5);
    assert_eq!(buffer.line_count(), 1);
    assert_eq!(buffer.version().get(), 0);
    assert!(!buffer.is_dirty());
}

#[test]
fn empty_buffer_has_one_line() {
    let buffer = Buffer::new(BufferConfig::default()).unwrap();

    assert_eq!(buffer.text(), "");
    assert_eq!(buffer.len_bytes(), ByteOffset::ZERO);
    assert_eq!(buffer.line_count(), 1);
    assert_eq!(buffer.line_start(Line::ZERO).unwrap(), ByteOffset::ZERO);
    assert_eq!(
        buffer.byte_to_position(ByteOffset::ZERO).unwrap(),
        Position::ZERO
    );
}

#[test]
fn insert_updates_text_version_dirty_state_and_line_index() {
    let mut buffer = Buffer::from_text("helo".to_string(), BufferConfig::default()).unwrap();

    let before = buffer.version();

    buffer.insert(ByteOffset::new(2), "l").unwrap();

    assert_eq!(buffer.text(), "hello");
    assert_eq!(buffer.version().get(), before.get() + 1);
    assert!(buffer.is_dirty());
    assert_eq!(buffer.line_count(), 1);
}

#[test]
fn insert_at_start_and_end() {
    let mut buffer = Buffer::from_text("middle".to_string(), BufferConfig::default()).unwrap();

    buffer.insert(ByteOffset::ZERO, "start-").unwrap();
    assert_eq!(buffer.text(), "start-middle");
    assert_eq!(buffer.version().get(), 1);

    buffer
        .insert(ByteOffset::new(buffer.len_bytes().get()), "-end")
        .unwrap();

    assert_eq!(buffer.text(), "start-middle-end");
    assert_eq!(buffer.version().get(), 2);
    assert!(buffer.is_dirty());
}

#[test]
fn replace_empty_range_behaves_like_insert() {
    let mut buffer = Buffer::from_text("helo".to_string(), BufferConfig::default()).unwrap();

    buffer.replace(range(2, 2), "l").unwrap();

    assert_eq!(buffer.text(), "hello");
    assert_eq!(buffer.version().get(), 1);
    assert!(buffer.is_dirty());
}

#[test]
fn delete_removes_text_range() {
    let mut buffer = Buffer::from_text("abcXYZdef".to_string(), BufferConfig::default()).unwrap();

    buffer.delete(range(3, 6)).unwrap();

    assert_eq!(buffer.text(), "abcdef");
    assert_eq!(buffer.version().get(), 1);
    assert!(buffer.is_dirty());
}

#[test]
fn delete_empty_range_is_no_op() {
    let mut buffer = Buffer::from_text("hello".to_string(), BufferConfig::default()).unwrap();

    buffer.delete(range(2, 2)).unwrap();

    assert_buffer_state(&buffer, "hello", 0, false, 1);
}

#[test]
fn delete_entire_text_leaves_single_empty_line() {
    let mut buffer =
        Buffer::from_text("hello\nworld".to_string(), BufferConfig::default()).unwrap();

    let len = buffer.len_bytes().get();
    buffer.delete(range(0, len)).unwrap();

    assert_buffer_state(&buffer, "", 1, true, 1);
    assert_eq!(buffer.line_start(Line::ZERO).unwrap(), ByteOffset::ZERO);
}

#[test]
fn replace_replaces_text_range() {
    let mut buffer = Buffer::from_text("hello world".to_string(), BufferConfig::default()).unwrap();

    buffer.replace(range(6, 11), "zom").unwrap();

    assert_eq!(buffer.text(), "hello zom");
    assert_eq!(buffer.version().get(), 1);
    assert!(buffer.is_dirty());
}

#[test]
fn replace_entire_text_rebuilds_line_index() {
    let mut buffer = Buffer::from_text("one line".to_string(), BufferConfig::default()).unwrap();

    let len = buffer.len_bytes().get();
    buffer.replace(range(0, len), "a\nb\n").unwrap();

    assert_eq!(buffer.text(), "a\nb\n");
    assert_eq!(buffer.line_count(), 3);
    assert_eq!(buffer.line_start(Line::new(0)).unwrap().get(), 0);
    assert_eq!(buffer.line_start(Line::new(1)).unwrap().get(), 2);
    assert_eq!(buffer.line_start(Line::new(2)).unwrap().get(), 4);
}

#[test]
fn no_op_replace_does_not_bump_version_or_dirty_state() {
    let mut buffer = Buffer::from_text("hello".to_string(), BufferConfig::default()).unwrap();

    let before = buffer.version();

    buffer.replace(range(1, 4), "ell").unwrap();

    assert_eq!(buffer.text(), "hello");
    assert_eq!(buffer.version(), before);
    assert!(!buffer.is_dirty());
}

#[test]
fn insert_newline_rebuilds_line_index() {
    let mut buffer = Buffer::from_text("ab".to_string(), BufferConfig::default()).unwrap();

    buffer.insert(ByteOffset::new(1), "\n").unwrap();

    assert_eq!(buffer.text(), "a\nb");
    assert_eq!(buffer.line_count(), 2);
    assert_eq!(buffer.line_start(Line::new(0)).unwrap().get(), 0);
    assert_eq!(buffer.line_start(Line::new(1)).unwrap().get(), 2);
    assert_eq!(
        buffer.byte_to_position(ByteOffset::new(2)).unwrap(),
        Position::new(Line::new(1), LogicalColumn::ZERO)
    );
}

#[test]
fn delete_newline_rebuilds_line_index() {
    let mut buffer = Buffer::from_text("a\nb".to_string(), BufferConfig::default()).unwrap();

    buffer.delete(range(1, 2)).unwrap();

    assert_eq!(buffer.text(), "ab");
    assert_eq!(buffer.line_count(), 1);
    assert_eq!(buffer.line_start(Line::ZERO).unwrap(), ByteOffset::ZERO);
    assert_eq!(
        buffer.byte_to_position(ByteOffset::new(2)).unwrap(),
        Position::new(Line::ZERO, LogicalColumn::new(2))
    );
}

#[test]
fn range_out_of_bounds_returns_edit_error() {
    let mut buffer = Buffer::from_text("hello".to_string(), BufferConfig::default()).unwrap();

    let err = buffer.delete(range(0, 99)).unwrap_err();

    assert!(matches!(
        err,
        EngineError::Edit(EditError::RangeOutOfBounds { .. })
    ));
}

#[test]
fn invalid_utf8_boundary_returns_coordinate_error() {
    let mut buffer = Buffer::from_text("你".to_string(), BufferConfig::default()).unwrap();

    let err = buffer.insert(ByteOffset::new(1), "x").unwrap_err();

    assert!(matches!(
        err,
        EngineError::Coordinate(CoordinateError::InvalidUtf8Boundary(_))
    ));
}

#[test]
fn failed_edit_does_not_change_text_version_dirty_or_line_index() {
    let mut buffer = Buffer::from_text("你\na".to_string(), BufferConfig::default()).unwrap();

    let before_text = buffer.text().to_string();
    let before_version = buffer.version();
    let before_saved_version = buffer.saved_version();
    let before_dirty = buffer.is_dirty();
    let before_line_count = buffer.line_count();
    let before_second_line_start = buffer.line_start(Line::new(1)).unwrap();

    let err = buffer.insert(ByteOffset::new(1), "x").unwrap_err();

    assert!(matches!(
        err,
        EngineError::Coordinate(CoordinateError::InvalidUtf8Boundary(_))
    ));
    assert_eq!(buffer.text(), before_text);
    assert_eq!(buffer.version(), before_version);
    assert_eq!(buffer.saved_version(), before_saved_version);
    assert_eq!(buffer.is_dirty(), before_dirty);
    assert_eq!(buffer.line_count(), before_line_count);
    assert_eq!(
        buffer.line_start(Line::new(1)).unwrap(),
        before_second_line_start
    );
}

#[test]
fn final_newline_creates_empty_last_line() {
    let buffer = Buffer::from_text("a\n".to_string(), BufferConfig::default()).unwrap();

    assert_eq!(buffer.line_count(), 2);
    assert_eq!(buffer.line_start(Line::new(0)).unwrap().get(), 0);
    assert_eq!(buffer.line_start(Line::new(1)).unwrap().get(), 2);
    assert_eq!(
        buffer.byte_to_position(ByteOffset::new(2)).unwrap(),
        Position::new(Line::new(1), LogicalColumn::ZERO)
    );
}

#[test]
fn multiple_lines_have_correct_starts() {
    let buffer = Buffer::from_text("a\nbb\nccc".to_string(), BufferConfig::default()).unwrap();

    assert_eq!(buffer.line_count(), 3);
    assert_eq!(buffer.line_start(Line::new(0)).unwrap().get(), 0);
    assert_eq!(buffer.line_start(Line::new(1)).unwrap().get(), 2);
    assert_eq!(buffer.line_start(Line::new(2)).unwrap().get(), 5);
}

#[test]
fn line_start_out_of_bounds_returns_coordinate_error() {
    let buffer = Buffer::from_text("a\nb".to_string(), BufferConfig::default()).unwrap();

    let err = buffer.line_start(Line::new(2)).unwrap_err();

    assert!(matches!(
        err,
        EngineError::Coordinate(CoordinateError::LineOutOfBounds(line)) if line == Line::new(2)
    ));
}

#[test]
fn crlf_line_starts_use_lf_as_next_line_start() {
    let buffer = Buffer::from_text("a\r\nb".to_string(), BufferConfig::default()).unwrap();

    assert_eq!(buffer.line_count(), 2);
    assert_eq!(buffer.line_start(Line::new(0)).unwrap().get(), 0);
    assert_eq!(buffer.line_start(Line::new(1)).unwrap().get(), 3);
    assert_eq!(
        buffer.byte_to_position(ByteOffset::new(3)).unwrap(),
        Position::new(Line::new(1), LogicalColumn::ZERO)
    );
}

#[test]
fn crlf_middle_is_not_a_valid_coordinate_or_edit_boundary() {
    let mut buffer = Buffer::from_text("a\r\nb".to_string(), BufferConfig::default()).unwrap();

    let coordinate_err = buffer.byte_to_position(ByteOffset::new(2)).unwrap_err();
    assert!(matches!(
        coordinate_err,
        EngineError::Coordinate(CoordinateError::InvalidUtf8Boundary(offset))
            if offset == ByteOffset::new(2)
    ));

    let edit_err = buffer.insert(ByteOffset::new(2), "x").unwrap_err();
    assert!(matches!(
        edit_err,
        EngineError::Edit(EditError::InvalidBoundary { offset })
            if offset == ByteOffset::new(2)
    ));

    assert_eq!(buffer.text(), "a\r\nb");
    assert_eq!(buffer.version().get(), 0);
    assert!(!buffer.is_dirty());
}

#[test]
fn byte_to_position_uses_logical_unicode_scalar_column() {
    let buffer = Buffer::from_text("你a好".to_string(), BufferConfig::default()).unwrap();

    assert_eq!(
        buffer.byte_to_position(ByteOffset::new(0)).unwrap(),
        Position::new(Line::new(0), LogicalColumn::new(0))
    );

    assert_eq!(
        buffer
            .byte_to_position(ByteOffset::new("你".len()))
            .unwrap(),
        Position::new(Line::new(0), LogicalColumn::new(1))
    );

    assert_eq!(
        buffer
            .byte_to_position(ByteOffset::new("你a".len()))
            .unwrap(),
        Position::new(Line::new(0), LogicalColumn::new(2))
    );
}

#[test]
fn byte_to_position_accepts_end_of_text() {
    let buffer = Buffer::from_text("abc".to_string(), BufferConfig::default()).unwrap();

    assert_eq!(
        buffer.byte_to_position(ByteOffset::new(3)).unwrap(),
        Position::new(Line::ZERO, LogicalColumn::new(3))
    );
}

#[test]
fn byte_to_position_rejects_offset_past_end() {
    let buffer = Buffer::from_text("abc".to_string(), BufferConfig::default()).unwrap();

    let err = buffer.byte_to_position(ByteOffset::new(4)).unwrap_err();

    assert!(matches!(
        err,
        EngineError::Coordinate(CoordinateError::OutOfBounds(offset))
            if offset == ByteOffset::new(4)
    ));
}

#[test]
fn position_to_byte_uses_logical_unicode_scalar_column() {
    let buffer = Buffer::from_text("你a好".to_string(), BufferConfig::default()).unwrap();

    assert_eq!(
        buffer
            .position_to_byte(Position::new(Line::new(0), LogicalColumn::new(0)))
            .unwrap()
            .get(),
        0
    );

    assert_eq!(
        buffer
            .position_to_byte(Position::new(Line::new(0), LogicalColumn::new(1)))
            .unwrap()
            .get(),
        "你".len()
    );

    assert_eq!(
        buffer
            .position_to_byte(Position::new(Line::new(0), LogicalColumn::new(2)))
            .unwrap()
            .get(),
        "你a".len()
    );
}

#[test]
fn position_to_byte_rejects_line_out_of_bounds() {
    let buffer = Buffer::from_text("a\nb".to_string(), BufferConfig::default()).unwrap();

    let err = buffer
        .position_to_byte(Position::new(Line::new(2), LogicalColumn::ZERO))
        .unwrap_err();

    assert!(matches!(
        err,
        EngineError::Coordinate(CoordinateError::LineOutOfBounds(line)) if line == Line::new(2)
    ));
}

#[test]
fn position_to_byte_rejects_column_past_line_end() {
    let buffer = Buffer::from_text("abc".to_string(), BufferConfig::default()).unwrap();

    let err = buffer
        .position_to_byte(Position::new(Line::ZERO, LogicalColumn::new(4)))
        .unwrap_err();

    assert!(matches!(
        err,
        EngineError::Coordinate(CoordinateError::OutOfBounds(_))
    ));
}

#[test]
fn position_to_byte_does_not_count_line_ending_as_content_column() {
    let buffer = Buffer::from_text("a\r\nb".to_string(), BufferConfig::default()).unwrap();

    assert_eq!(
        buffer
            .position_to_byte(Position::new(Line::ZERO, LogicalColumn::new(1)))
            .unwrap(),
        ByteOffset::new(1)
    );

    let err = buffer
        .position_to_byte(Position::new(Line::ZERO, LogicalColumn::new(2)))
        .unwrap_err();

    assert!(matches!(
        err,
        EngineError::Coordinate(CoordinateError::OutOfBounds(_))
    ));
}

#[test]
fn byte_position_roundtrip() {
    let buffer = Buffer::from_text("你a\n好b".to_string(), BufferConfig::default()).unwrap();

    for offset in [0, "你".len(), "你a".len(), "你a\n".len(), "你a\n好".len()] {
        let byte = ByteOffset::new(offset);
        let position = buffer.byte_to_position(byte).unwrap();
        let roundtrip = buffer.position_to_byte(position).unwrap();

        assert_eq!(roundtrip, byte);
    }
}

#[test]
fn mark_saved_clears_dirty_state() {
    let mut buffer = Buffer::from_text("hello".to_string(), BufferConfig::default()).unwrap();

    buffer.insert(ByteOffset::new(5), "!").unwrap();
    assert!(buffer.is_dirty());

    buffer.mark_saved();
    assert!(!buffer.is_dirty());
    assert_eq!(buffer.saved_version(), buffer.version());
}

#[test]
fn edit_after_mark_saved_marks_dirty_again() {
    let mut buffer = Buffer::from_text("hello".to_string(), BufferConfig::default()).unwrap();

    buffer.insert(ByteOffset::new(5), "!").unwrap();
    buffer.mark_saved();
    assert!(!buffer.is_dirty());

    buffer.delete(range(5, 6)).unwrap();

    assert_eq!(buffer.text(), "hello");
    assert!(buffer.is_dirty());
    assert_eq!(buffer.version().get(), buffer.saved_version().get() + 1);
}
