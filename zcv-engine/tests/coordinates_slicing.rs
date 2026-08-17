use zcv_engine::*;
pub mod common;
use common::*;

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
fn flat_utf16_cu_should_roundtrip_against_byte_offsets_across_planes_and_newlines() {
    // 文本内容：
    //   "a" — 1 字节 / 1 UTF-16 cu
    //   "你" — 3 字节 / 1 UTF-16 cu
    //   "\n" — 1 字节 / 1 UTF-16 cu
    //   "😀" — 4 字节 / 2 UTF-16 cu（BMP 外，surrogate pair）
    //   "z" — 1 字节 / 1 UTF-16 cu
    let buffer = buffer("a你\n😀z");

    // 累计 byte → utf16 cu。
    assert_eq!(buffer.byte_to_utf16_cu(b(0)).unwrap(), Utf16Offset::new(0));
    assert_eq!(buffer.byte_to_utf16_cu(b(1)).unwrap(), Utf16Offset::new(1)); // 跨过 "a"
    assert_eq!(buffer.byte_to_utf16_cu(b(4)).unwrap(), Utf16Offset::new(2)); // 跨过 "你"
    assert_eq!(buffer.byte_to_utf16_cu(b(5)).unwrap(), Utf16Offset::new(3)); // 跨过 "\n"
    assert_eq!(buffer.byte_to_utf16_cu(b(9)).unwrap(), Utf16Offset::new(5)); // 跨过 "😀"
    assert_eq!(buffer.byte_to_utf16_cu(b(10)).unwrap(), Utf16Offset::new(6)); // 跨过 "z"

    // 反向回放：每个 utf16 cu 边界回到对应 byte。
    for (utf16, byte) in [(0, 0), (1, 1), (2, 4), (3, 5), (5, 9), (6, 10)] {
        assert_eq!(
            buffer.utf16_cu_to_byte(Utf16Offset::new(utf16)).unwrap(),
            b(byte)
        );
    }

    // 落在 surrogate pair 中间（utf16=4）→ InvalidUtf16Boundary。
    assert!(matches!(
        buffer.utf16_cu_to_byte(Utf16Offset::new(4)).unwrap_err(),
        EngineError::Coordinate(CoordinateError::InvalidUtf16Boundary(_))
    ));

    // 越界 → Utf16PositionOutOfBounds。
    assert!(matches!(
        buffer.utf16_cu_to_byte(Utf16Offset::new(999)).unwrap_err(),
        EngineError::Coordinate(CoordinateError::Utf16PositionOutOfBounds(_))
    ));
}

#[test]
fn flat_utf16_cu_should_work_on_empty_buffer_and_be_zero_at_origin() {
    let buffer = buffer("");
    assert_eq!(buffer.byte_to_utf16_cu(b(0)).unwrap(), Utf16Offset::new(0));
    assert_eq!(buffer.utf16_cu_to_byte(Utf16Offset::new(0)).unwrap(), b(0));
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
    let byte_err = buffer
        .edit(
            [Edit::insert(b(2), "x").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap_err();

    assert!(matches!(
        char_err,
        EngineError::Coordinate(CoordinateError::CharOutOfBounds(offset)) if offset == c(2)
    ));
    assert!(matches!(
        byte_err,
        EngineError::Edit(EditError::InvalidBoundary { offset }) if offset == b(2)
    ));
    assert_eq!(buffer_text(&buffer), "a\r\nb");
    assert_eq!(buffer.line_start(line(1)).unwrap(), c(3));
    assert_eq!(buffer.line_start_byte(line(1)).unwrap(), b(3));
}

#[test]
fn trailing_empty_line_position_should_project_to_document_end() {
    let buffer = buffer("aa\n");

    assert_eq!(buffer.line_count(), 2);
    assert_eq!(
        buffer.char_to_position(c(3)).unwrap(),
        Position::new(line(1), col(0))
    );
    assert_eq!(
        buffer
            .position_to_char(Position::new(line(1), col(0)))
            .unwrap(),
        c(3)
    );
    assert_eq!(
        buffer
            .utf16_position_to_char(Utf16Position::new(line(1), Utf16Offset::new(0)))
            .unwrap(),
        c(3)
    );
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
fn snapshot_coordinate_and_slicing_queries_should_read_old_version_after_state_transition() {
    let mut buffer = buffer("alpha\nbeta");
    let snapshot = buffer.snapshot();

    buffer
        .edit(
            [Edit::replace(range(6, 10), "BETA")],
            TransactionMetadata::default(),
        )
        .unwrap();

    assert_eq!(buffer_text(&snapshot), "alpha\nbeta");
    assert_eq!(snapshot.slice_line(line(1)).unwrap().as_str(), "beta");
    assert_eq!(
        snapshot
            .position_to_byte(Position::new(line(1), col(2)))
            .unwrap(),
        b(8)
    );
    assert_eq!(buffer.slice_line(line(1)).unwrap().as_str(), "BETA");
}
