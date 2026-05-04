use zom_engine::*;

#[test]
fn m4_ropey_backed_buffer_keeps_existing_public_edit_contract() -> EngineResult<()> {
    let mut buffer = Buffer::from_text("hello\n世界".to_string(), BufferConfig::default())?;

    buffer.insert(CharOffset::new(5), "🙂")?;
    assert_eq!(buffer.text().as_ref(), "hello🙂\n世界");

    buffer.delete(TextRange::new(CharOffset::new(5), CharOffset::new(6))?)?;
    assert_eq!(buffer.text().as_ref(), "hello\n世界");

    buffer.replace(
        TextRange::new(CharOffset::new(6), CharOffset::new(8))?,
        "Ropey",
    )?;
    assert_eq!(buffer.text().as_ref(), "hello\nRopey");

    assert_eq!(buffer.len_chars(), CharOffset::new(11));
    assert_eq!(buffer.line_count(), 2);
    assert_eq!(
        buffer.position_to_char(Position::new(Line::new(1), LogicalColumn::new(0)))?,
        CharOffset::new(6)
    );

    Ok(())
}

#[test]
fn m4_snapshot_is_versioned_and_immutable() -> EngineResult<()> {
    let mut buffer = Buffer::from_text("one\ntwo".to_string(), BufferConfig::default())?;
    let snapshot = buffer.snapshot();

    buffer.replace(
        TextRange::new(CharOffset::new(4), CharOffset::new(7))?,
        "TWO",
    )?;

    assert_eq!(snapshot.text().as_ref(), "one\ntwo");
    assert_eq!(buffer.text().as_ref(), "one\nTWO");
    assert!(snapshot.is_stale_for(&buffer));
    assert!(buffer.is_snapshot_stale(&snapshot));

    Ok(())
}

#[test]
fn m4_large_and_long_line_basic_smoke() -> EngineResult<()> {
    let mut text = "a".repeat(200_000);
    text.push('\n');
    text.push_str(&"b".repeat(200_000));

    let mut buffer = Buffer::from_text(text, BufferConfig::default())?;
    buffer.insert(CharOffset::new(100_000), "中🙂")?;
    buffer.replace(
        TextRange::new(CharOffset::new(200_003), CharOffset::new(200_003))?,
        "tail",
    )?;

    assert_eq!(buffer.line_count(), 2);
    assert_eq!(buffer.line_start(Line::new(1))?, CharOffset::new(200_003));

    Ok(())
}

#[test]
fn m4_crlf_boundary_is_still_rejected() -> EngineResult<()> {
    let mut buffer = Buffer::from_text("a\r\nb".to_string(), BufferConfig::default())?;

    let result = buffer.insert(CharOffset::new(2), "X");
    assert!(result.is_err());
    assert_eq!(buffer.text().as_ref(), "a\r\nb");
    assert_eq!(buffer.version(), BufferVersion::INITIAL);

    Ok(())
}

#[test]
fn m4_ropey_buffer_matches_string_reference_for_basic_edits() -> EngineResult<()> {
    let initial = "hello\n中文🙂\r\nlast".to_string();
    let mut buffer = Buffer::from_text(initial.clone(), BufferConfig::default())?;
    let mut reference = StringReference::new(initial);

    assert_buffer_matches_reference(&buffer, &reference)?;

    replace_both(
        &mut buffer,
        &mut reference,
        TextRange::new(CharOffset::new(5), CharOffset::new(5))?,
        " world",
    )?;

    replace_both(
        &mut buffer,
        &mut reference,
        TextRange::new(CharOffset::new(0), CharOffset::new(1))?,
        "H",
    )?;

    replace_both(
        &mut buffer,
        &mut reference,
        TextRange::new(CharOffset::new(8), CharOffset::new(10))?,
        "",
    )?;

    replace_both(
        &mut buffer,
        &mut reference,
        TextRange::new(CharOffset::new(3), CharOffset::new(7))?,
        "🙂中",
    )?;

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StringReference {
    text: String,
}

impl StringReference {
    fn new(text: String) -> Self {
        Self { text }
    }

    fn text(&self) -> &str {
        &self.text
    }

    fn replace(&mut self, range: TextRange, replacement: &str) -> EngineResult<()> {
        if range.end().get() > self.len_chars().get() {
            return Err(EditError::RangeOutOfBounds { range }.into());
        }

        let start = self.char_to_byte_index(range.start())?;
        let end = self.char_to_byte_index(range.end())?;
        self.text.replace_range(start..end, replacement);
        Ok(())
    }

    fn len_bytes(&self) -> usize {
        self.text.len()
    }

    fn len_chars(&self) -> CharOffset {
        CharOffset::new(self.text.chars().count())
    }

    fn len_utf16_cu(&self) -> usize {
        self.text.encode_utf16().count()
    }

    fn line_count(&self) -> usize {
        self.line_starts().len()
    }

    fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        self.line_starts()
            .get(line.get())
            .copied()
            .map(CharOffset::new)
            .ok_or_else(|| CoordinateError::LineOutOfBounds(line).into())
    }

    fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position> {
        let offset_value = offset.get();
        let text_len = self.len_chars().get();

        if offset_value > text_len {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        if self.is_crlf_middle(offset_value) {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        let starts = self.line_starts();
        let line_index = starts
            .partition_point(|start| *start <= offset_value)
            .saturating_sub(1);
        let line_start = starts[line_index];
        let column = offset_value - line_start;

        Ok(Position::new(
            Line::new(line_index),
            LogicalColumn::new(column),
        ))
    }

    fn position_to_char(&self, position: Position) -> EngineResult<CharOffset> {
        let line = position.line();
        let column = position.column().get();
        let starts = self.line_starts();

        let Some(line_start) = starts.get(line.get()).copied() else {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        };

        let next_line_start = starts
            .get(line.get() + 1)
            .copied()
            .unwrap_or_else(|| self.len_chars().get());
        let line_content_end = self.line_content_end(next_line_start);
        let line_len = line_content_end - line_start;

        if column <= line_len {
            return Ok(CharOffset::new(line_start + column));
        }

        Err(CoordinateError::OutOfBounds(CharOffset::new(line_content_end)).into())
    }

    fn char_to_byte_index(&self, offset: CharOffset) -> EngineResult<usize> {
        let char_offset = offset.get();
        let len_chars = self.len_chars().get();

        if char_offset > len_chars {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        if char_offset == len_chars {
            return Ok(self.text.len());
        }

        self.text
            .char_indices()
            .nth(char_offset)
            .map(|(byte_idx, _)| byte_idx)
            .ok_or_else(|| CoordinateError::OutOfBounds(offset).into())
    }

    fn line_starts(&self) -> Vec<usize> {
        let mut starts = vec![0];

        for (char_idx, ch) in self.text.chars().enumerate() {
            if ch == '\n' {
                starts.push(char_idx + 1);
            }
        }

        starts
    }

    fn line_content_end(&self, next_line_start: usize) -> usize {
        if next_line_start == 0 {
            return next_line_start;
        }

        let Some(prev) = self.text.chars().nth(next_line_start - 1) else {
            return next_line_start;
        };

        if prev != '\n' {
            return next_line_start;
        }

        let before_newline = next_line_start - 1;
        if before_newline == 0 {
            return before_newline;
        }

        let Some(before_prev) = self.text.chars().nth(before_newline - 1) else {
            return before_newline;
        };

        if before_prev == '\r' {
            before_newline - 1
        } else {
            before_newline
        }
    }

    fn is_crlf_middle(&self, offset: usize) -> bool {
        if offset == 0 || offset >= self.len_chars().get() {
            return false;
        }

        matches!(
            (
                self.text.chars().nth(offset - 1),
                self.text.chars().nth(offset)
            ),
            (Some('\r'), Some('\n'))
        )
    }
}

fn replace_both(
    buffer: &mut Buffer,
    reference: &mut StringReference,
    range: TextRange,
    replacement: &str,
) -> EngineResult<()> {
    buffer.replace(range, replacement)?;
    reference.replace(range, replacement)?;
    assert_buffer_matches_reference(buffer, reference)
}

fn assert_buffer_matches_reference(
    buffer: &Buffer,
    reference: &StringReference,
) -> EngineResult<()> {
    assert_eq!(buffer.text().as_ref(), reference.text());
    assert_eq!(buffer.len_bytes(), reference.len_bytes());
    assert_eq!(buffer.len_chars(), reference.len_chars());
    assert_eq!(buffer.len_utf16_cu(), reference.len_utf16_cu());
    assert_eq!(buffer.line_count(), reference.line_count());

    for line_idx in 0..buffer.line_count() {
        let line = Line::new(line_idx);
        assert_eq!(buffer.line_start(line)?, reference.line_start(line)?);
    }

    for char_idx in 0..=buffer.len_chars().get() {
        let offset = CharOffset::new(char_idx);
        assert_eq!(
            buffer.char_to_position(offset),
            reference.char_to_position(offset)
        );

        if let Ok(position) = reference.char_to_position(offset) {
            assert_eq!(buffer.position_to_char(position)?, offset);
            assert_eq!(reference.position_to_char(position)?, offset);
        }
    }

    Ok(())
}
