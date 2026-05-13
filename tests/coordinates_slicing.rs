use zom_engine::*;

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

fn line(value: usize) -> Line {
    Line::new(value)
}

fn col(value: usize) -> LogicalColumn {
    LogicalColumn::new(value)
}

fn dcol(value: usize) -> DisplayColumn {
    DisplayColumn::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

fn line_range(start: usize, end: usize) -> LineRange {
    LineRange::new(line(start), line(end)).unwrap()
}

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

#[test]
fn byte_char_position_utf16_roundtrip_should_preserve_explicit_coordinate_domains() {
    let buffer = buffer("a你\n😀z");

    assert_eq!(buffer.char_to_byte(c(0)).unwrap(), b(0));
    assert_eq!(buffer.char_to_byte(c(1)).unwrap(), b(1));
    assert_eq!(buffer.char_to_byte(c(2)).unwrap(), b(4));
    assert_eq!(buffer.byte_to_char(b(9)).unwrap(), c(4));
    assert_eq!(
        buffer.byte_to_position(b(9)).unwrap(),
        Position::new(line(1), col(1))
    );
    assert_eq!(
        buffer
            .position_to_byte(Position::new(line(1), col(1)))
            .unwrap(),
        b(9)
    );
    assert_eq!(
        buffer.char_to_utf16_position(c(4)).unwrap(),
        Utf16Position::new(line(1), Utf16Offset::new(2))
    );
    assert_eq!(
        buffer
            .utf16_position_to_byte(Utf16Position::new(line(1), Utf16Offset::new(2)))
            .unwrap(),
        b(9)
    );
}

#[test]
fn invalid_utf8_byte_boundary_should_be_rejected_by_byte_projection_and_slice() {
    let buffer = buffer("你a");

    let coordinate = buffer.byte_to_position(b(1)).unwrap_err();
    let slice = buffer.slice_text(range(1, 2)).unwrap_err();

    assert!(matches!(
        coordinate,
        EngineError::Coordinate(CoordinateError::InvalidByteBoundary(offset)) if offset == b(1)
    ));
    assert!(matches!(
        slice,
        EngineError::Coordinate(CoordinateError::InvalidByteBoundary(offset)) if offset == b(1)
    ));
}

#[test]
fn crlf_middle_should_not_be_valid_line_position_or_edit_boundary() {
    let mut buffer = buffer("a\r\nb");

    let char_err = buffer.char_to_position(c(2)).unwrap_err();
    let byte_err = buffer.insert(b(2), "x").unwrap_err();

    assert!(matches!(
        char_err,
        EngineError::Coordinate(CoordinateError::CharOutOfBounds(offset)) if offset == c(2)
    ));
    assert!(matches!(
        byte_err,
        EngineError::Edit(EditError::InvalidBoundary { offset }) if offset == b(2)
    ));
    assert_eq!(buffer.text().as_ref(), "a\r\nb");
    assert_eq!(buffer.line_start(line(1)).unwrap(), c(3));
    assert_eq!(buffer.line_start_byte(line(1)).unwrap(), b(3));
}

#[test]
fn grapheme_boundary_should_reject_combining_mark_middle_and_map_to_nearest_boundaries() {
    let buffer = buffer("ae\u{301}b");

    assert!(buffer.is_grapheme_boundary(c(1)).unwrap());
    assert!(!buffer.is_grapheme_boundary(c(2)).unwrap());
    assert!(matches!(
        buffer.validate_grapheme_boundary(c(2)).unwrap_err(),
        EngineError::Coordinate(CoordinateError::InvalidGraphemeBoundary(offset)) if offset == b(2)
    ));
    assert_eq!(buffer.previous_grapheme_boundary(c(3)).unwrap(), c(1));
    assert_eq!(buffer.next_grapheme_boundary(c(1)).unwrap(), c(3));
}

#[test]
fn display_column_mapping_should_expand_tabs_and_width_policy_without_line_ending_columns() {
    let buffer = Buffer::from_text(
        "\t你a\nb".to_string(),
        BufferConfig {
            tab: TabConfig::new(
                std::num::NonZeroUsize::new(4).unwrap(),
                std::num::NonZeroUsize::new(4).unwrap(),
                true,
            ),
            ..BufferConfig::default()
        },
    )
    .unwrap();

    assert_eq!(buffer.next_tab_stop(dcol(1)), dcol(4));
    assert_eq!(
        buffer.logical_to_display_column(line(0), col(1)).unwrap(),
        dcol(4)
    );
    assert_eq!(
        buffer.logical_to_display_column(line(0), col(2)).unwrap(),
        dcol(6)
    );
    assert_eq!(buffer.char_to_display_column(c(2)).unwrap(), dcol(6));
    assert_eq!(
        buffer
            .display_to_logical_column_with_affinity(
                line(0),
                dcol(5),
                DisplayColumnAffinity::Previous
            )
            .unwrap(),
        col(1)
    );
}

#[test]
fn text_and_line_slices_should_preserve_byte_ranges_and_newline_boundaries() {
    let buffer = buffer("one\n二三\nlast");
    let slice = buffer.slice_text(range(4, 10)).unwrap();
    let line_slice = buffer.slice_line(line(1)).unwrap();
    let range_slice = buffer.slice_line_range(line_range(1, 3)).unwrap();

    assert_eq!(slice.as_str(), "二三");
    assert_eq!(slice.range(), range(4, 10));
    assert_eq!(line_slice.as_str(), "二三\n");
    assert_eq!(line_slice.range(), range(4, 11));
    assert_eq!(range_slice.as_str(), "二三\nlast");
}

#[test]
fn viewport_slice_should_clamp_end_and_truncate_by_unicode_scalar_count() {
    let buffer = buffer("short\n你好吗\nlast");
    let viewport = Viewport::new(line(1), 8).with_max_line_chars(2);
    let slice = buffer.slice_viewport(viewport).unwrap();

    assert_eq!(slice.line_range(), line_range(1, 3));
    assert_eq!(slice.len(), 2);
    assert_eq!(slice.lines()[0].as_str(), "你好");
    assert!(slice.lines()[0].is_truncated());
    assert_eq!(slice.lines()[0].visible_len_chars(), 2);
    assert_eq!(slice.lines()[1].as_str(), "la");
}

#[test]
fn snapshot_coordinate_and_slicing_queries_should_read_old_version_after_state_transition() {
    let mut buffer = buffer("alpha\nbeta");
    let snapshot = buffer.snapshot();

    buffer.replace(range(6, 10), "BETA").unwrap();

    assert_eq!(snapshot.text().as_ref(), "alpha\nbeta");
    assert_eq!(snapshot.slice_line(line(1)).unwrap().as_str(), "beta");
    assert_eq!(
        snapshot
            .position_to_byte(Position::new(line(1), col(2)))
            .unwrap(),
        b(8)
    );
    assert_eq!(buffer.slice_line(line(1)).unwrap().as_str(), "BETA");
}
