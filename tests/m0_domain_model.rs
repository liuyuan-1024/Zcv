use std::num::NonZeroUsize;

use zom_engine::{
    BufferConfig, BufferVersion, ByteOffset, CharOffset, CoordinateError, DisplayColumn,
    EngineError, Line, LineEndingConfig, LogicalColumn, Position, PositionEncodingConfig,
    TabConfig, TextRange, TransactionId, Utf16Offset,
};

#[test]
fn root_public_api_can_be_imported() {
    let _ = ByteOffset::new(0);
    let _ = CharOffset::new(0);
    let _ = Utf16Offset::new(0);

    let _ = Line::new(0);
    let _ = LogicalColumn::new(0);
    let _ = DisplayColumn::new(0);

    let _ = Position::new(Line::new(0), LogicalColumn::new(0));
    let _ = TextRange::new(ByteOffset::new(0), ByteOffset::new(0)).unwrap();

    let _ = BufferVersion::INITIAL;
    let _ = TransactionId::INITIAL;

    let _ = BufferConfig::default();
}

#[test]
fn coordinate_newtypes_are_zero_indexed() {
    assert_eq!(ByteOffset::ZERO.get(), 0);
    assert_eq!(CharOffset::ZERO.get(), 0);
    assert_eq!(Utf16Offset::ZERO.get(), 0);

    assert_eq!(Line::ZERO.get(), 0);
    assert_eq!(LogicalColumn::ZERO.get(), 0);
    assert_eq!(DisplayColumn::ZERO.get(), 0);
}

#[test]
fn position_is_a_zero_indexed_logical_text_position() {
    let position = Position::new(Line::new(2), LogicalColumn::new(4));

    assert_eq!(position.line, Line::new(2));
    assert_eq!(position.column, LogicalColumn::new(4));
}

#[test]
fn text_range_accepts_ordered_offsets() {
    let range = TextRange::new(ByteOffset::new(1), ByteOffset::new(3)).unwrap();

    assert_eq!(range.start(), ByteOffset::new(1));
    assert_eq!(range.end(), ByteOffset::new(3));
    assert_eq!(range.len(), 2);
    assert!(!range.is_empty());
}

#[test]
fn text_range_accepts_empty_range() {
    let range = TextRange::new(ByteOffset::new(2), ByteOffset::new(2)).unwrap();

    assert_eq!(range.start(), ByteOffset::new(2));
    assert_eq!(range.end(), ByteOffset::new(2));
    assert_eq!(range.len(), 0);
    assert!(range.is_empty());
}

#[test]
fn text_range_rejects_reversed_offsets() {
    let err = TextRange::new(ByteOffset::new(3), ByteOffset::new(1)).unwrap_err();

    assert_eq!(
        err,
        CoordinateError::InvalidRange {
            start: ByteOffset::new(3),
            end: ByteOffset::new(1),
        }
    );
}

#[test]
fn text_range_new_unchecked_constructs_range() {
    let range = TextRange::new_unchecked(ByteOffset::new(1), ByteOffset::new(3));

    assert_eq!(range.start(), ByteOffset::new(1));
    assert_eq!(range.end(), ByteOffset::new(3));
    assert_eq!(range.len(), 2);
    assert!(!range.is_empty());
}

#[test]
fn byte_offset_checked_arithmetic_does_not_panic() {
    assert_eq!(ByteOffset::new(3).checked_add(2), Some(ByteOffset::new(5)));
    assert_eq!(ByteOffset::new(3).checked_sub(2), Some(ByteOffset::new(1)));
    assert_eq!(ByteOffset::new(0).checked_sub(1), None);
}

#[test]
fn buffer_version_can_advance() {
    assert_eq!(BufferVersion::INITIAL.get(), 0);
    assert_eq!(BufferVersion::INITIAL.next(), Some(BufferVersion::new(1)));
}

#[test]
fn transaction_id_can_advance() {
    assert_eq!(TransactionId::INITIAL.get(), 0);
    assert_eq!(TransactionId::INITIAL.next(), Some(TransactionId::new(1)));
}

#[test]
fn default_buffer_config_is_reasonable_for_m0() {
    let config = BufferConfig::default();

    assert_eq!(config.line_ending, LineEndingConfig::Preserve);
    assert_eq!(config.position_encoding, PositionEncodingConfig::Utf8);

    assert_eq!(config.tab.tab_width(), 4);
    assert_eq!(config.tab.indent_width(), 4);
    assert!(config.tab.insert_spaces);

    assert!(config.large_file.threshold_bytes > 0);
    assert!(config.large_file.long_line_threshold_bytes > 0);
}

#[test]
fn tab_config_requires_non_zero_widths_at_type_level() {
    let config = TabConfig::new(
        NonZeroUsize::new(2).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        true,
    );

    assert_eq!(config.tab_width(), 2);
    assert_eq!(config.indent_width(), 4);
    assert!(config.insert_spaces);
}

#[test]
fn coordinate_error_can_be_lifted_to_engine_error() {
    let error = CoordinateError::InvalidRange {
        start: ByteOffset::new(3),
        end: ByteOffset::new(1),
    };

    let engine_error: EngineError = error.into();

    assert!(matches!(engine_error, EngineError::Coordinate(_)));
}
