use std::num::NonZeroUsize;

use zom_engine::*;

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
fn text_range_half_open_boundary_should_report_len_empty_overlap_and_contains() {
    let empty = TextRange::new(b(4), b(4)).unwrap();
    let left = TextRange::new(b(2), b(5)).unwrap();
    let adjacent = TextRange::new(b(5), b(9)).unwrap();
    let overlapping = TextRange::new(b(4), b(8)).unwrap();

    assert!(empty.is_empty());
    assert_eq!(left.len(), 3);
    assert!(left.contains(b(2)));
    assert!(!left.contains(b(5)));
    assert!(!left.overlaps(adjacent));
    assert!(left.overlaps(overlapping));
}

#[test]
fn line_range_reversed_lines_should_return_invalid_line_range() {
    let err = LineRange::new(line(4), line(1)).unwrap_err();

    assert!(matches!(
        err,
        CoordinateError::InvalidLineRange { start, end } if start == line(4) && end == line(1)
    ));
}

#[test]
fn offsets_checked_and_saturating_arithmetic_should_not_panic_at_usize_edges() {
    assert_eq!(b(4).checked_add(3), Some(b(7)));
    assert_eq!(b(4).checked_sub(5), None);
    assert_eq!(ByteOffset::new(usize::MAX).checked_add(1), None);
    assert_eq!(b(4).saturating_sub(9), ByteOffset::ZERO);
    assert_eq!(
        ByteOffset::new(usize::MAX).saturating_add(1),
        ByteOffset::new(usize::MAX)
    );
    assert_eq!(CharOffset::new(4).checked_sub(5), None);
}

#[test]
fn versions_and_transaction_ids_should_advance_until_overflow_boundary() {
    assert_eq!(BufferVersion::INITIAL.next(), Some(BufferVersion::new(1)));
    assert_eq!(TransactionId::INITIAL.next(), Some(TransactionId::new(1)));
    assert_eq!(BufferVersion::new(u64::MAX).next(), None);
    assert_eq!(TransactionId::new(u64::MAX).next(), None);
}

#[test]
fn origin_handle_should_remain_host_opaque() {
    let anonymous = BufferOrigin::anonymous();
    let external = BufferOrigin::external("zom://opaque/path");

    assert_eq!(anonymous.kind(), OriginKind::Anonymous);
    assert_eq!(anonymous.handle(), None);
    assert!(anonymous.is_anonymous());
    assert_eq!(external.kind(), OriginKind::External);
    assert_eq!(external.handle(), Some("zom://opaque/path"));
    assert!(!external.is_anonymous());
}

#[test]
fn config_strategy_values_should_expose_stable_defaults_and_boundaries() {
    let config = BufferConfig::default();
    let tab = TabConfig::new(
        NonZeroUsize::new(2).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        true,
    );
    let large = LargeFilePolicy {
        large_file_threshold_bytes: 8,
        long_line_threshold_chars: 3,
        ..LargeFilePolicy::default()
    };

    assert_eq!(config.tab.tab_width(), 4);
    assert_eq!(config.tab.indent_width(), 4);
    assert_eq!(tab.tab_width(), 2);
    assert_eq!(tab.indent_width(), 4);
    assert!(tab.insert_spaces);
    assert!(large.is_large_byte_size(9));
    assert!(!large.is_large_byte_size(8));
    assert!(large.is_long_line(4));
    assert!(!large.is_long_line(3));
}

#[test]
fn domain_errors_should_lift_into_engine_error_without_losing_variant() {
    let coordinate: EngineError = CoordinateError::OutOfBounds(b(99)).into();
    let edit: EngineError = EditError::PayloadTooLarge { size: 9, limit: 3 }.into();
    let transaction: EngineError = TransactionError::EmptyTransaction.into();

    assert!(matches!(
        coordinate,
        EngineError::Coordinate(CoordinateError::OutOfBounds(offset)) if offset == b(99)
    ));
    assert!(matches!(
        edit,
        EngineError::Edit(EditError::PayloadTooLarge { size: 9, limit: 3 })
    ));
    assert!(matches!(
        transaction,
        EngineError::Transaction(TransactionError::EmptyTransaction)
    ));
}
