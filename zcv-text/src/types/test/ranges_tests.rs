use super::*;

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn line(value: usize) -> Line {
    Line::new(value)
}

#[test]
fn text_range_reversed_byte_offsets_should_return_invalid_range() {
    let err = TextRange::new(b(8), b(3)).unwrap_err();

    assert!(matches!(
        err,
        CoordinateError::InvalidRange { start, end } if start == b(8) && end == b(3)
    ));
}

#[test]
fn text_range_half_open_boundary_should_report_len_empty_and_contains() {
    let empty = TextRange::new(b(4), b(4)).unwrap();
    let left = TextRange::new(b(2), b(5)).unwrap();

    assert!(empty.is_empty());
    assert_eq!(left.len(), 3);
    assert!(left.contains(b(2)));
    assert!(!left.contains(b(5)));
}

#[test]
fn line_range_reversed_lines_should_return_invalid_line_range() {
    let err = LineRange::new(line(4), line(1)).unwrap_err();

    assert!(matches!(
        err,
        CoordinateError::InvalidLineRange { start, end } if start == line(4) && end == line(1)
    ));
}
