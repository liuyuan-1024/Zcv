#[path = "common/buffer.rs"]
mod buffer;
#[path = "common/char_offset.rs"]
mod char_offset;

use buffer::buffer;
use char_offset::c;
use zcv_text::{CoordinateError, MovementDirection, MovementUnit, TextError, WordBoundaryPolicy};

#[test]
fn movement_boundaries_dispatch_by_unit_and_reject_invalid_offsets() {
    let buffer = buffer("foo_barBaz42 += 世界");

    assert_eq!(
        buffer
            .movement_boundary(
                WordBoundaryPolicy::default(),
                c(0),
                MovementDirection::Next,
                MovementUnit::Word
            )
            .unwrap(),
        c(3)
    );
    assert_eq!(
        buffer
            .movement_boundary(
                WordBoundaryPolicy::default(),
                c(0),
                MovementDirection::Next,
                MovementUnit::Identifier
            )
            .unwrap(),
        c(12)
    );
    assert_eq!(
        buffer
            .movement_boundary(
                WordBoundaryPolicy::default(),
                c(0),
                MovementDirection::Next,
                MovementUnit::Subword
            )
            .unwrap(),
        c(3)
    );
    assert_eq!(
        buffer
            .movement_boundary(
                WordBoundaryPolicy::default(),
                c(13),
                MovementDirection::Next,
                MovementUnit::Symbol
            )
            .unwrap(),
        c(15)
    );
    assert!(
        buffer
            .movement_boundary(
                WordBoundaryPolicy::default(),
                c(99),
                MovementDirection::Next,
                MovementUnit::Word
            )
            .is_err()
    );
}

#[test]
fn grapheme_boundaries_reject_the_middle_of_a_cluster() {
    let buffer = buffer("e\u{301}x");

    assert!(buffer.is_grapheme_boundary(c(0)).unwrap());
    assert!(!buffer.is_grapheme_boundary(c(1)).unwrap());
    assert!(matches!(
        buffer.validate_grapheme_boundary(c(1)).unwrap_err(),
        TextError::Coordinate(CoordinateError::InvalidGraphemeBoundary(_))
    ));
    assert_eq!(buffer.next_grapheme_boundary(c(0)).unwrap(), c(2));
}

#[test]
fn word_boundaries_keep_newline_as_an_independent_category() {
    let buffer = buffer("abc\n\n  next");
    let empty_line_start = c("abc\n".chars().count());

    assert_eq!(
        buffer
            .movement_boundary(
                WordBoundaryPolicy::default(),
                empty_line_start,
                MovementDirection::Next,
                MovementUnit::Word,
            )
            .unwrap(),
        c("abc\n\n".chars().count())
    );
    assert_eq!(
        buffer
            .movement_boundary(
                WordBoundaryPolicy::default(),
                empty_line_start,
                MovementDirection::Previous,
                MovementUnit::Word,
            )
            .unwrap(),
        c("abc".chars().count())
    );
}
