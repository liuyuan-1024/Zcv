//! M11 机器契约：锁定 Viewport Slicing 与读取接口的 public 行为。
//!
//! M11A 只验证 LineRange、TextSlice、LineSlice 与按 char/byte/line range 读取；
//! 不测试 UI 渲染、viewport 投影、折叠或滚动体感。

mod m11a_line_range_and_text_slicing {
    use zom_engine::{
        Buffer, BufferConfig, ByteOffset, CharOffset, CoordinateError, EngineError, Line,
        LineRange, LineSlice, TextRange, TextSlice,
    };

    fn buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    fn c(value: usize) -> CharOffset {
        CharOffset::new(value)
    }

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(c(start), c(end)).unwrap()
    }

    fn line_range(start: usize, end: usize) -> LineRange {
        LineRange::new(Line::new(start), Line::new(end)).unwrap()
    }

    #[test]
    fn slice_types_are_public_crate_root_api() {
        fn accepts_text_slice(_: Option<TextSlice<'static>>) {}
        fn accepts_line_slice(_: Option<LineSlice<'static>>) {}

        accepts_text_slice(None);
        accepts_line_slice(None);
    }

    #[test]
    fn buffer_can_slice_text_by_char_range() {
        let buffer = buffer("aé\n中b");

        let slice = buffer.slice_text(range(1, 4)).unwrap();

        assert_eq!(slice.range(), range(1, 4));
        assert_eq!(slice.as_str(), "é\n中");
        assert_eq!(slice.len_chars(), 3);
        assert_eq!(slice.len_bytes(), "é\n中".len());
        assert!(!slice.is_empty());
        assert_eq!(slice.to_string(), "é\n中");
    }

    #[test]
    fn buffer_line_slice_preserves_exact_line_text_and_range() {
        let buffer = buffer("aa\r\nbb\ncc");

        let first = buffer.slice_line(Line::new(0)).unwrap();
        let second = buffer.slice_line(Line::new(1)).unwrap();
        let third = buffer.slice_line(Line::new(2)).unwrap();

        assert_eq!(first.line(), Line::new(0));
        assert_eq!(first.range(), range(0, 4));
        assert_eq!(first.as_str(), "aa\r\n");
        assert_eq!(first.len_chars(), 4);

        assert_eq!(second.line(), Line::new(1));
        assert_eq!(second.range(), range(4, 7));
        assert_eq!(second.as_str(), "bb\n");

        assert_eq!(third.line(), Line::new(2));
        assert_eq!(third.range(), range(7, 9));
        assert_eq!(third.as_str(), "cc");
    }

    #[test]
    fn buffer_can_slice_text_by_half_open_line_range() {
        let buffer = buffer("aa\nbb\ncc");

        let slice = buffer.slice_line_range(line_range(1, 3)).unwrap();
        let empty_at_document_end = buffer.slice_line_range(line_range(3, 3)).unwrap();

        assert_eq!(slice.range(), range(3, 8));
        assert_eq!(slice.as_str(), "bb\ncc");
        assert_eq!(empty_at_document_end.range(), range(8, 8));
        assert_eq!(empty_at_document_end.as_str(), "");
        assert!(empty_at_document_end.is_empty());
    }

    #[test]
    fn line_slicing_validates_against_buffer_line_boundaries() {
        let buffer = buffer("aa\nbb");

        let missing_line = buffer.slice_line(Line::new(2)).unwrap_err();
        let missing_boundary = buffer.slice_line_range(line_range(0, 3)).unwrap_err();

        assert_eq!(
            missing_line,
            EngineError::Coordinate(CoordinateError::LineOutOfBounds(Line::new(2)))
        );
        assert_eq!(
            missing_boundary,
            EngineError::Coordinate(CoordinateError::LineOutOfBounds(Line::new(3)))
        );
    }

    #[test]
    fn buffer_can_slice_text_by_utf8_byte_range() {
        let buffer = buffer("aé中b");

        let slice = buffer
            .slice_byte_range(ByteOffset::new(1), ByteOffset::new(6))
            .unwrap();

        assert_eq!(slice.range(), range(1, 3));
        assert_eq!(slice.as_str(), "é中");
        assert_eq!(slice.len_chars(), 2);
        assert_eq!(slice.len_bytes(), "é中".len());
    }

    #[test]
    fn byte_range_slicing_rejects_reversed_or_invalid_utf8_boundaries() {
        let buffer = buffer("aé");

        let reversed = buffer
            .slice_byte_range(ByteOffset::new(3), ByteOffset::new(1))
            .unwrap_err();
        let invalid_boundary = buffer
            .slice_byte_range(ByteOffset::new(1), ByteOffset::new(2))
            .unwrap_err();

        assert_eq!(
            reversed,
            EngineError::Coordinate(CoordinateError::InvalidByteRange {
                start: ByteOffset::new(3),
                end: ByteOffset::new(1),
            })
        );
        assert_eq!(
            invalid_boundary,
            EngineError::Coordinate(CoordinateError::InvalidByteBoundary(ByteOffset::new(2)))
        );
    }

    #[test]
    fn snapshot_slicing_reads_the_snapshot_version_not_the_mutated_buffer() {
        let mut buffer = buffer("one\ntwo");
        let snapshot = buffer.snapshot();

        buffer.insert(CharOffset::ZERO, "zero\n").unwrap();

        let snapshot_slice = snapshot.slice_line_range(line_range(0, 1)).unwrap();
        let buffer_slice = buffer.slice_line_range(line_range(0, 1)).unwrap();

        assert_eq!(snapshot_slice.as_str(), "one\n");
        assert_eq!(snapshot_slice.range(), range(0, 4));
        assert_eq!(buffer_slice.as_str(), "zero\n");
        assert!(buffer.is_snapshot_stale(&snapshot));
    }
}

mod m11b_viewport_reading {
    use zom_engine::{
        Buffer, BufferConfig, CharOffset, CoordinateError, EngineError, Line, LineRange, TextRange,
        Viewport, ViewportSlice, VisibleLine,
    };

    fn buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    fn c(value: usize) -> CharOffset {
        CharOffset::new(value)
    }

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(c(start), c(end)).unwrap()
    }

    fn line_range(start: usize, end: usize) -> LineRange {
        LineRange::new(Line::new(start), Line::new(end)).unwrap()
    }

    #[test]
    fn viewport_types_are_public_crate_root_api() {
        fn accepts_viewport(_: Viewport) {}
        fn accepts_viewport_slice(_: Option<ViewportSlice<'static>>) {}
        fn accepts_visible_line(_: Option<VisibleLine<'static>>) {}

        accepts_viewport(Viewport::new(Line::ZERO, 0));
        accepts_viewport_slice(None);
        accepts_visible_line(None);
    }

    #[test]
    fn buffer_can_slice_visible_lines_by_viewport() {
        let buffer = buffer("aa\nbb\ncc\ndd");
        let viewport = Viewport::new(Line::new(1), 2);

        let slice = buffer.slice_viewport(viewport).unwrap();
        let lines = slice.lines();

        assert_eq!(slice.viewport(), viewport);
        assert_eq!(slice.line_range(), line_range(1, 3));
        assert_eq!(slice.len(), 2);
        assert_eq!(lines[0].line(), Line::new(1));
        assert_eq!(lines[0].full_range(), range(3, 6));
        assert_eq!(lines[0].visible_range(), range(3, 5));
        assert_eq!(lines[0].as_str(), "bb");
        assert_eq!(lines[0].full_len_chars(), 3);
        assert_eq!(lines[0].visible_len_chars(), 2);
        assert!(!lines[0].is_truncated());

        assert_eq!(lines[1].line(), Line::new(2));
        assert_eq!(lines[1].full_range(), range(6, 9));
        assert_eq!(lines[1].visible_range(), range(6, 8));
        assert_eq!(lines[1].as_str(), "cc");
    }

    #[test]
    fn viewport_clamps_to_document_end_for_stable_scrolling() {
        let buffer = buffer("aa\nbb\ncc");

        let slice = buffer
            .slice_viewport(Viewport::new(Line::new(2), 20))
            .unwrap();
        let empty_at_end = buffer
            .slice_viewport(Viewport::new(Line::new(3), 10))
            .unwrap();

        assert_eq!(slice.line_range(), line_range(2, 3));
        assert_eq!(slice.lines().len(), 1);
        assert_eq!(slice.lines()[0].as_str(), "cc");
        assert_eq!(empty_at_end.line_range(), line_range(3, 3));
        assert!(empty_at_end.is_empty());
    }

    #[test]
    fn viewport_rejects_start_line_past_document_boundary() {
        let buffer = buffer("aa\nbb");

        let err = buffer
            .slice_viewport(Viewport::new(Line::new(3), 1))
            .unwrap_err();

        assert_eq!(
            err,
            EngineError::Coordinate(CoordinateError::LineOutOfBounds(Line::new(3)))
        );
    }

    #[test]
    fn viewport_can_limit_visible_chars_per_long_line() {
        let buffer = buffer("0123456789\nshort");
        let viewport = Viewport::new(Line::ZERO, 2).with_max_line_chars(4);

        let slice = buffer.slice_viewport(viewport).unwrap();
        let lines = slice.lines();

        assert_eq!(slice.viewport().max_line_chars(), Some(4));
        assert_eq!(lines[0].full_range(), range(0, 11));
        assert_eq!(lines[0].visible_range(), range(0, 4));
        assert_eq!(lines[0].as_str(), "0123");
        assert!(lines[0].is_truncated());

        assert_eq!(lines[1].full_range(), range(11, 16));
        assert_eq!(lines[1].visible_range(), range(11, 15));
        assert_eq!(lines[1].as_str(), "shor");
        assert!(lines[1].is_truncated());
    }

    #[test]
    fn viewport_without_line_limit_returns_full_visible_line_content() {
        let buffer = buffer("0123456789\nshort");
        let viewport = Viewport::new(Line::ZERO, 1)
            .with_max_line_chars(4)
            .without_line_limit();

        let slice = buffer.slice_viewport(viewport).unwrap();

        assert_eq!(slice.viewport().max_line_chars(), None);
        assert_eq!(slice.lines()[0].visible_range(), range(0, 10));
        assert_eq!(slice.lines()[0].as_str(), "0123456789");
        assert!(!slice.lines()[0].is_truncated());
    }

    #[test]
    fn snapshot_viewport_reads_snapshot_text_after_buffer_mutation() {
        let mut buffer = buffer("one\ntwo\nthree");
        let snapshot = buffer.snapshot();

        buffer.insert(CharOffset::ZERO, "zero\n").unwrap();

        let snapshot_slice = snapshot
            .slice_viewport(Viewport::new(Line::new(1), 2))
            .unwrap();
        let buffer_slice = buffer
            .slice_viewport(Viewport::new(Line::new(1), 2))
            .unwrap();

        assert_eq!(
            snapshot_slice
                .lines()
                .iter()
                .map(|line| line.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "three"]
        );
        assert_eq!(
            buffer_slice
                .lines()
                .iter()
                .map(|line| line.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        assert!(buffer.is_snapshot_stale(&snapshot));
    }

    #[test]
    fn viewport_slicing_handles_large_line_windows_near_document_end() {
        let text = (0..2_000)
            .map(|line| format!("line-{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let buffer = buffer(&text);

        let slice = buffer
            .slice_viewport(Viewport::new(Line::new(1_995), 20))
            .unwrap();

        assert_eq!(slice.line_range(), line_range(1_995, 2_000));
        assert_eq!(slice.lines().len(), 5);
        assert_eq!(slice.lines()[0].as_str(), "line-1995");
        assert_eq!(slice.lines()[4].as_str(), "line-1999");
    }
}
