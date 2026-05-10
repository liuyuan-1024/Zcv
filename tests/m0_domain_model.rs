//! M0 机器契约：锁定基础领域类型、配置对象和错误枚举的 public API 语义。
//!
//! 本文件只测试类型边界和不变量，不涉及 Buffer 编辑、事务、历史或 GPUI 体感。

use std::num::NonZeroUsize;

use zom_engine::{
    BomPolicy, BufferConfig, BufferId, BufferKind, BufferState, BufferVersion, ByteOffset,
    CharOffset, CoordinateError, DisplayColumn, EditError, EncodingConfig, EngineError,
    InvalidUtf8Policy, Line, LineEndingConfig, LineRange, LogicalColumn, Position,
    PositionEncodingConfig, StorageError, TabConfig, TextEncoding, TextRange, TransactionError,
    TransactionId, Utf16Offset,
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
    let _ = TextRange::new(CharOffset::new(0), CharOffset::new(0)).unwrap();
    let _ = LineRange::new(Line::new(0), Line::new(0)).unwrap();

    let _ = BufferVersion::INITIAL;
    let _ = BufferId::INITIAL;
    let _ = BufferKind::Untitled;
    let _ = BufferState::Clean;
    let _ = TransactionId::INITIAL;
    let _ = TextEncoding::Utf8;
    let _ = BomPolicy::Strip;
    let _ = InvalidUtf8Policy::Reject;

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
fn coordinate_newtypes_can_be_ordered_and_compared() {
    assert!(ByteOffset::new(1) < ByteOffset::new(2));
    assert!(CharOffset::new(1) < CharOffset::new(2));
    assert!(Utf16Offset::new(1) < Utf16Offset::new(2));

    assert!(Line::new(1) < Line::new(2));
    assert!(LogicalColumn::new(1) < LogicalColumn::new(2));
    assert!(DisplayColumn::new(1) < DisplayColumn::new(2));
}

#[test]
fn position_is_a_zero_indexed_logical_text_position() {
    let position = Position::new(Line::new(2), LogicalColumn::new(4));

    assert_eq!(position.line(), Line::new(2));
    assert_eq!(position.column(), LogicalColumn::new(4));
}

#[test]
fn position_zero_is_line_zero_column_zero() {
    assert_eq!(Position::ZERO.line(), Line::ZERO);
    assert_eq!(Position::ZERO.column(), LogicalColumn::ZERO);
}

#[test]
fn text_range_accepts_ordered_char_offsets() {
    let range = TextRange::new(CharOffset::new(1), CharOffset::new(3)).unwrap();

    assert_eq!(range.start(), CharOffset::new(1));
    assert_eq!(range.end(), CharOffset::new(3));
    assert_eq!(range.len(), 2);
    assert!(!range.is_empty());
}

#[test]
fn text_range_accepts_empty_char_range() {
    let range = TextRange::new(CharOffset::new(2), CharOffset::new(2)).unwrap();

    assert_eq!(range.start(), CharOffset::new(2));
    assert_eq!(range.end(), CharOffset::new(2));
    assert_eq!(range.len(), 0);
    assert!(range.is_empty());
}

#[test]
fn text_range_rejects_reversed_char_offsets() {
    let err = TextRange::new(CharOffset::new(3), CharOffset::new(1)).unwrap_err();

    assert_eq!(
        err,
        CoordinateError::InvalidRange {
            start: CharOffset::new(3),
            end: CharOffset::new(1),
        }
    );
}

#[test]
fn text_range_constructor_is_the_only_public_range_constructor() {
    let ok = TextRange::new(CharOffset::new(1), CharOffset::new(1));
    let err = TextRange::new(CharOffset::new(2), CharOffset::new(1));

    assert!(ok.is_ok());
    assert!(matches!(err, Err(CoordinateError::InvalidRange { .. })));
}

#[test]
fn line_range_accepts_ordered_half_open_lines() {
    let range = LineRange::new(Line::new(1), Line::new(3)).unwrap();

    assert_eq!(range.start(), Line::new(1));
    assert_eq!(range.end(), Line::new(3));
    assert_eq!(range.len(), 2);
    assert!(!range.is_empty());
}

#[test]
fn line_range_accepts_empty_line_range() {
    let range = LineRange::new(Line::new(2), Line::new(2)).unwrap();

    assert_eq!(range.start(), Line::new(2));
    assert_eq!(range.end(), Line::new(2));
    assert_eq!(range.len(), 0);
    assert!(range.is_empty());
}

#[test]
fn line_range_rejects_reversed_lines() {
    let err = LineRange::new(Line::new(3), Line::new(1)).unwrap_err();

    assert_eq!(
        err,
        CoordinateError::InvalidLineRange {
            start: Line::new(3),
            end: Line::new(1),
        }
    );
}

#[test]
fn byte_offset_checked_arithmetic_does_not_panic() {
    assert_eq!(ByteOffset::new(3).checked_add(2), Some(ByteOffset::new(5)));
    assert_eq!(ByteOffset::new(3).checked_sub(2), Some(ByteOffset::new(1)));
    assert_eq!(ByteOffset::new(0).checked_sub(1), None);
}

#[test]
fn byte_offset_checked_arithmetic_reports_overflow() {
    assert_eq!(ByteOffset::new(usize::MAX).checked_add(1), None);
}

#[test]
fn char_offset_checked_arithmetic_does_not_panic() {
    assert_eq!(CharOffset::new(3).checked_add(2), Some(CharOffset::new(5)));
    assert_eq!(CharOffset::new(3).checked_sub(2), Some(CharOffset::new(1)));
    assert_eq!(CharOffset::new(0).checked_sub(1), None);
}

#[test]
fn char_offset_checked_arithmetic_reports_overflow() {
    assert_eq!(CharOffset::new(usize::MAX).checked_add(1), None);
}

#[test]
fn buffer_version_starts_at_initial_and_can_advance() {
    assert_eq!(BufferVersion::default(), BufferVersion::INITIAL);
    assert_eq!(BufferVersion::INITIAL.get(), 0);
    assert_eq!(BufferVersion::INITIAL.next(), Some(BufferVersion::new(1)));
}

#[test]
fn buffer_version_reports_overflow() {
    assert_eq!(BufferVersion::new(u64::MAX).next(), None);
}

#[test]
fn transaction_id_starts_at_initial_and_can_advance() {
    assert_eq!(TransactionId::default(), TransactionId::INITIAL);
    assert_eq!(TransactionId::INITIAL.get(), 0);
    assert_eq!(TransactionId::INITIAL.next(), Some(TransactionId::new(1)));
}

#[test]
fn transaction_id_reports_overflow() {
    assert_eq!(TransactionId::new(u64::MAX).next(), None);
}

#[test]
fn default_buffer_config_is_reasonable_for_m0() {
    let config = BufferConfig::default();

    assert_eq!(config.line_ending, LineEndingConfig::Preserve);
    assert_eq!(config.encoding, EncodingConfig::default());
    assert_eq!(config.position_encoding, PositionEncodingConfig::Utf8);

    assert_eq!(config.tab.tab_width(), 4);
    assert_eq!(config.tab.indent_width(), 4);
    assert!(config.tab.insert_spaces);

    assert!(config.large_file.max_undo_history > 0);
    assert!(config.large_file.max_undo_history_bytes > 0);
    assert!(config.large_file.large_transaction_threshold_bytes > 0);
    assert_eq!(
        config.large_file.large_transaction_policy,
        zom_engine::LargeTransactionPolicy::SkipHistory,
    );
    assert!(config.large_file.large_file_threshold_bytes > 0);
    assert!(config.large_file.long_line_threshold_chars > 0);
    assert!(!config.large_file.auto_read_only_on_large_file);
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
fn config_enums_are_public_strategy_values() {
    let _ = LineEndingConfig::Lf;
    let _ = LineEndingConfig::Crlf;
    let _ = LineEndingConfig::Preserve;
    let _ = LineEndingConfig::Native;

    let _ = PositionEncodingConfig::Utf8;
    let _ = PositionEncodingConfig::Utf16;
    let _ = PositionEncodingConfig::Utf32;

    let _ = BomPolicy::Strip;
    let _ = BomPolicy::Preserve;

    let _ = InvalidUtf8Policy::Reject;
    let _ = InvalidUtf8Policy::Replace;
}

#[test]
fn coordinate_error_can_be_lifted_to_engine_error() {
    let error = CoordinateError::InvalidRange {
        start: CharOffset::new(3),
        end: CharOffset::new(1),
    };

    let engine_error: EngineError = error.into();

    assert!(matches!(engine_error, EngineError::Coordinate(_)));
}

#[test]
fn edit_error_can_be_lifted_to_engine_error() {
    let range = TextRange::new(CharOffset::new(0), CharOffset::new(1)).unwrap();
    let error = EditError::RangeOutOfBounds { range };

    let engine_error: EngineError = error.into();

    assert!(matches!(engine_error, EngineError::Edit(_)));
}

#[test]
fn transaction_error_can_be_lifted_to_engine_error() {
    let error = TransactionError::VersionMismatch {
        expected: BufferVersion::new(2),
        actual: BufferVersion::new(1),
    };

    let engine_error: EngineError = error.into();

    assert!(matches!(engine_error, EngineError::Transaction(_)));
}

#[test]
fn storage_error_can_be_lifted_to_engine_error() {
    let error = StorageError::ReadOnly;

    let engine_error: EngineError = error.into();

    assert!(matches!(engine_error, EngineError::Storage(_)));
}
