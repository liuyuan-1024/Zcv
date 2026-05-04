use zom_engine::*;

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

#[test]
fn byte_and_char_offsets_roundtrip_on_multibyte_text() {
    let buffer = buffer("aé中😀b");

    for char_idx in 0..=buffer.len_chars().get() {
        let char_offset = CharOffset::new(char_idx);
        let byte_offset = buffer.char_to_byte(char_offset).unwrap();
        assert_eq!(buffer.byte_to_char(byte_offset).unwrap(), char_offset);
    }
}

#[test]
fn byte_to_char_rejects_offset_inside_utf8_codepoint() {
    let buffer = buffer("aé");

    let err = buffer.byte_to_char(ByteOffset::new(2)).unwrap_err();
    assert!(matches!(
        err,
        EngineError::Coordinate(CoordinateError::InvalidByteBoundary(offset)) if offset == ByteOffset::new(2)
    ));
}

#[test]
fn char_and_utf16_position_roundtrip_across_lines() {
    let buffer = buffer("a\n😀b");

    let after_emoji = CharOffset::new(3);
    let utf16 = buffer.char_to_utf16_position(after_emoji).unwrap();

    assert_eq!(utf16, Utf16Position::new(Line::new(1), Utf16Offset::new(2)));
    assert_eq!(buffer.utf16_position_to_char(utf16).unwrap(), after_emoji);
}

#[test]
fn utf16_position_rejects_middle_of_surrogate_pair() {
    let buffer = buffer("😀");

    let err = buffer
        .utf16_position_to_char(Utf16Position::new(Line::ZERO, Utf16Offset::new(1)))
        .unwrap_err();

    assert!(matches!(
        err,
        EngineError::Coordinate(CoordinateError::InvalidUtf16Boundary(_))
    ));
}

#[test]
fn byte_and_utf16_position_roundtrip() {
    let buffer = buffer("é\n😀b");
    let byte_offset = buffer.char_to_byte(CharOffset::new(3)).unwrap();

    let utf16 = buffer.byte_to_utf16_position(byte_offset).unwrap();

    assert_eq!(utf16, Utf16Position::new(Line::new(1), Utf16Offset::new(2)));
    assert_eq!(buffer.utf16_position_to_byte(utf16).unwrap(), byte_offset);
}

#[test]
fn grapheme_boundary_detects_combining_mark_cluster() {
    let buffer = buffer("a e\u{301} b");

    assert!(buffer.is_grapheme_boundary(CharOffset::new(2)).unwrap());
    assert!(!buffer.is_grapheme_boundary(CharOffset::new(3)).unwrap());
    assert!(buffer.is_grapheme_boundary(CharOffset::new(4)).unwrap());

    assert_eq!(
        buffer
            .previous_grapheme_boundary(CharOffset::new(3))
            .unwrap(),
        CharOffset::new(2)
    );
    assert_eq!(
        buffer.next_grapheme_boundary(CharOffset::new(3)).unwrap(),
        CharOffset::new(4)
    );
}

#[test]
fn strict_grapheme_boundary_validation_reports_invalid_boundary() {
    let buffer = buffer("e\u{301}");

    let err = buffer
        .validate_grapheme_boundary(CharOffset::new(1))
        .unwrap_err();

    assert!(matches!(
        err,
        EngineError::Coordinate(CoordinateError::InvalidGraphemeBoundary(offset)) if offset == CharOffset::new(1)
    ));
}

#[test]
fn crlf_middle_is_still_rejected_as_invalid_text_boundary() {
    let mut buffer = buffer("a\r\nb");

    assert!(buffer.char_to_position(CharOffset::new(2)).is_err());

    let err = buffer.insert(CharOffset::new(2), "x").unwrap_err();
    assert!(matches!(
        err,
        EngineError::Edit(EditError::InvalidBoundary { offset }) if offset == CharOffset::new(2)
    ));
}

#[test]
fn line_ending_style_is_detected() {
    assert_eq!(buffer("abc").line_ending_style(), LineEndingStyle::None);
    assert_eq!(buffer("a\nb\n").line_ending_style(), LineEndingStyle::Lf);
    assert_eq!(
        buffer("a\r\nb\r\n").line_ending_style(),
        LineEndingStyle::Crlf
    );
    assert_eq!(
        buffer("a\nb\r\n").line_ending_style(),
        LineEndingStyle::Mixed
    );
}

#[test]
fn snapshot_exposes_same_coordinate_conversions_as_buffer() {
    let buffer = buffer("a\n😀b");
    let snapshot = buffer.snapshot();

    let offset = CharOffset::new(3);
    let utf16 = snapshot.char_to_utf16_position(offset).unwrap();

    assert_eq!(utf16, Utf16Position::new(Line::new(1), Utf16Offset::new(2)));
    assert_eq!(snapshot.utf16_position_to_char(utf16).unwrap(), offset);
    assert_eq!(snapshot.line_ending_style(), LineEndingStyle::Lf);
}
