use zom_engine::{
    Buffer, BufferConfig, ByteOffset, CoordinateError, EditError, EngineError, Line, LogicalColumn,
    Position, TextRange,
};

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
    assert_eq!(buffer.line_count(), 1);
    assert_eq!(buffer.line_start(Line::ZERO).unwrap(), ByteOffset::ZERO);
}

#[test]
fn insert_updates_text_version_and_dirty_state() {
    let mut buffer = Buffer::from_text("helo".to_string(), BufferConfig::default()).unwrap();

    let before = buffer.version();

    buffer.insert(ByteOffset::new(2), "l").unwrap();

    assert_eq!(buffer.text(), "hello");
    assert_eq!(buffer.version().get(), before.get() + 1);
    assert!(buffer.is_dirty());
}

#[test]
fn delete_removes_text_range() {
    let mut buffer = Buffer::from_text("abcXYZdef".to_string(), BufferConfig::default()).unwrap();

    let range = TextRange::new(ByteOffset::new(3), ByteOffset::new(6)).unwrap();
    buffer.delete(range).unwrap();

    assert_eq!(buffer.text(), "abcdef");
}

#[test]
fn replace_replaces_text_range() {
    let mut buffer = Buffer::from_text("hello world".to_string(), BufferConfig::default()).unwrap();

    let range = TextRange::new(ByteOffset::new(6), ByteOffset::new(11)).unwrap();
    buffer.replace(range, "zom").unwrap();

    assert_eq!(buffer.text(), "hello zom");
}

#[test]
fn no_op_replace_does_not_bump_version_or_dirty_state() {
    let mut buffer = Buffer::from_text("hello".to_string(), BufferConfig::default()).unwrap();

    let before = buffer.version();
    let range = TextRange::new(ByteOffset::new(1), ByteOffset::new(4)).unwrap();

    buffer.replace(range, "ell").unwrap();

    assert_eq!(buffer.text(), "hello");
    assert_eq!(buffer.version(), before);
    assert!(!buffer.is_dirty());
}

#[test]
fn range_out_of_bounds_returns_edit_error() {
    let mut buffer = Buffer::from_text("hello".to_string(), BufferConfig::default()).unwrap();

    let range = TextRange::new(ByteOffset::new(0), ByteOffset::new(99)).unwrap();
    let err = buffer.delete(range).unwrap_err();

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
fn final_newline_creates_empty_last_line() {
    let buffer = Buffer::from_text("a\n".to_string(), BufferConfig::default()).unwrap();

    assert_eq!(buffer.line_count(), 2);
    assert_eq!(buffer.line_start(Line::new(0)).unwrap().get(), 0);
    assert_eq!(buffer.line_start(Line::new(1)).unwrap().get(), 2);
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
