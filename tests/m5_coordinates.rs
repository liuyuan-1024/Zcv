//! M5 机器契约：聚合 IDE 坐标转换、DisplayColumn、tab 展开和视觉列吸附行为。
//!
//! 小阶段测试保留在本文件的子模块中，避免一个大阶段拆出多个 cargo test 入口。

mod m5a_coordinate_conversions {
    //! M5A 机器契约：锁定 byte、char、UTF-16、grapheme 和换行风格之间的 IDE 坐标转换。
    //!
    //! 本文件验证坐标数学的 public 行为，不测试视觉列宽策略，也不进入 UI 渲染层。

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
}

mod m5b_display_columns {
    //! M5B 机器契约：锁定 DisplayColumn、tab 展开、字符宽度策略和视觉列吸附行为。
    //!
    //! 本文件只验证纯文本列宽数学，不承担真实字体测量、像素布局或 GPUI 光标绘制。

    use std::num::NonZeroUsize;

    use zom_engine::*;

    fn buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    fn buffer_with_tab_width(text: &str, tab_width: usize) -> Buffer {
        let config = BufferConfig {
            tab: TabConfig::new(
                NonZeroUsize::new(tab_width).unwrap(),
                NonZeroUsize::new(tab_width).unwrap(),
                true,
            ),
            ..BufferConfig::default()
        };

        Buffer::from_text(text.to_string(), config).unwrap()
    }

    #[test]
    fn next_tab_stop_uses_configured_tab_width() {
        let buffer = buffer_with_tab_width("", 4);

        assert_eq!(
            buffer.next_tab_stop(DisplayColumn::new(0)),
            DisplayColumn::new(4)
        );
        assert_eq!(
            buffer.next_tab_stop(DisplayColumn::new(1)),
            DisplayColumn::new(4)
        );
        assert_eq!(
            buffer.next_tab_stop(DisplayColumn::new(4)),
            DisplayColumn::new(8)
        );

        let buffer = buffer_with_tab_width("", 2);
        assert_eq!(
            buffer.next_tab_stop(DisplayColumn::new(1)),
            DisplayColumn::new(2)
        );
        assert_eq!(
            buffer.next_tab_stop(DisplayColumn::new(2)),
            DisplayColumn::new(4)
        );
    }

    #[test]
    fn logical_column_to_display_column_expands_tabs() {
        let buffer = buffer_with_tab_width("ab\tc", 4);

        assert_eq!(
            buffer
                .logical_to_display_column(Line::ZERO, LogicalColumn::new(0))
                .unwrap(),
            DisplayColumn::new(0)
        );
        assert_eq!(
            buffer
                .logical_to_display_column(Line::ZERO, LogicalColumn::new(1))
                .unwrap(),
            DisplayColumn::new(1)
        );
        assert_eq!(
            buffer
                .logical_to_display_column(Line::ZERO, LogicalColumn::new(2))
                .unwrap(),
            DisplayColumn::new(2)
        );
        assert_eq!(
            buffer
                .logical_to_display_column(Line::ZERO, LogicalColumn::new(3))
                .unwrap(),
            DisplayColumn::new(4)
        );
        assert_eq!(
            buffer
                .logical_to_display_column(Line::ZERO, LogicalColumn::new(4))
                .unwrap(),
            DisplayColumn::new(5)
        );
    }

    #[test]
    fn display_column_to_logical_column_has_explicit_tab_affinity() {
        let buffer = buffer_with_tab_width("ab\tc", 4);

        // The tab spans display columns 2..4. Column 3 is inside the tab expansion.
        assert_eq!(
            buffer
                .display_to_logical_column_with_affinity(
                    Line::ZERO,
                    DisplayColumn::new(3),
                    DisplayColumnAffinity::Previous,
                )
                .unwrap(),
            LogicalColumn::new(2)
        );
        assert_eq!(
            buffer
                .display_to_logical_column_with_affinity(
                    Line::ZERO,
                    DisplayColumn::new(3),
                    DisplayColumnAffinity::Next,
                )
                .unwrap(),
            LogicalColumn::new(3)
        );
        assert_eq!(
            buffer
                .display_to_logical_column(Line::ZERO, DisplayColumn::new(3))
                .unwrap(),
            LogicalColumn::new(2)
        );
    }

    #[test]
    fn display_column_to_char_clamps_to_line_end_when_target_is_past_content() {
        let buffer = buffer("abc\ndef");

        assert_eq!(
            buffer
                .display_to_logical_column(Line::ZERO, DisplayColumn::new(99))
                .unwrap(),
            LogicalColumn::new(3)
        );
        assert_eq!(
            buffer
                .display_column_to_char(Line::ZERO, DisplayColumn::new(99))
                .unwrap(),
            CharOffset::new(3)
        );
    }

    #[test]
    fn cjk_and_emoji_widths_follow_display_width_policy() {
        let buffer = buffer("a中😀b");

        assert_eq!(
            buffer.char_to_display_column(CharOffset::new(0)).unwrap(),
            DisplayColumn::new(0)
        );
        assert_eq!(
            buffer.char_to_display_column(CharOffset::new(1)).unwrap(),
            DisplayColumn::new(1)
        );
        assert_eq!(
            buffer.char_to_display_column(CharOffset::new(2)).unwrap(),
            DisplayColumn::new(3)
        );
        assert_eq!(
            buffer.char_to_display_column(CharOffset::new(3)).unwrap(),
            DisplayColumn::new(5)
        );
        assert_eq!(
            buffer.char_to_display_column(CharOffset::new(4)).unwrap(),
            DisplayColumn::new(6)
        );
    }

    #[test]
    fn combining_marks_have_zero_default_display_width() {
        let buffer = buffer("e\u{301}x");

        assert_eq!(
            buffer.char_to_display_column(CharOffset::new(0)).unwrap(),
            DisplayColumn::new(0)
        );
        assert_eq!(
            buffer.char_to_display_column(CharOffset::new(1)).unwrap(),
            DisplayColumn::new(1)
        );
        assert_eq!(
            buffer.char_to_display_column(CharOffset::new(2)).unwrap(),
            DisplayColumn::new(1)
        );
        assert_eq!(
            buffer.char_to_display_column(CharOffset::new(3)).unwrap(),
            DisplayColumn::new(2)
        );
    }

    #[test]
    fn display_width_math_is_per_line_and_ignores_line_endings() {
        let buffer = buffer_with_tab_width("a\t\r\n中", 4);

        assert_eq!(
            buffer
                .logical_to_display_column(Line::ZERO, LogicalColumn::new(2))
                .unwrap(),
            DisplayColumn::new(4)
        );
        assert_eq!(
            buffer
                .logical_to_display_column(Line::new(1), LogicalColumn::new(1))
                .unwrap(),
            DisplayColumn::new(2)
        );
    }

    #[test]
    fn invalid_logical_position_still_returns_coordinate_error() {
        let buffer = buffer("abc");

        assert!(
            buffer
                .logical_to_display_column(Line::ZERO, LogicalColumn::new(4))
                .is_err()
        );
        assert!(
            buffer
                .display_to_logical_column(Line::new(9), DisplayColumn::new(0))
                .is_err()
        );
    }

    #[test]
    fn snapshot_preserves_display_width_policy_and_text() {
        let mut buffer = buffer_with_tab_width("a\tb", 4);
        let snapshot = buffer.snapshot();

        buffer.insert(CharOffset::ZERO, "xx").unwrap();

        assert_eq!(
            snapshot
                .logical_to_display_column(Line::ZERO, LogicalColumn::new(2))
                .unwrap(),
            DisplayColumn::new(4)
        );
        assert_eq!(snapshot.text().as_ref(), "a\tb");
    }
}
