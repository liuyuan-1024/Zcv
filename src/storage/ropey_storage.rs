use std::borrow::Cow;

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

use super::{TextRead, TextStorage};
use crate::{
    ByteOffset, CharOffset, CoordinateError, EditError, EngineResult, Line, LineEndingStyle,
    LogicalColumn, Position, TextRange, Utf16Offset, Utf16Position,
};

/// M4 默认高性能文本后端。
///
/// 不把 `ropey::Rope` 暴露到 public API；外部仍然只看到 Buffer / Snapshot / CharOffset。
#[derive(Debug, Clone)]
pub(crate) struct RopeyStorage {
    rope: Rope,
}

impl RopeyStorage {
    pub(crate) fn new(text: String) -> Self {
        Self {
            rope: Rope::from_str(&text),
        }
    }

    fn validate_range(&self, range: TextRange) -> EngineResult<()> {
        if range.end().get() > self.rope.len_chars() {
            return Err(EditError::RangeOutOfBounds { range }.into());
        }

        Ok(())
    }
}

impl TextRead for RopeyStorage {
    fn text(&self) -> Cow<'_, str> {
        Cow::Owned(self.rope.to_string())
    }

    fn slice_text(&self, range: TextRange) -> EngineResult<Cow<'_, str>> {
        self.validate_range(range)?;
        Ok(Cow::Owned(
            self.rope
                .slice(range.start().get()..range.end().get())
                .to_string(),
        ))
    }

    fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    fn len_chars(&self) -> CharOffset {
        CharOffset::new(self.rope.len_chars())
    }

    fn len_utf16_cu(&self) -> usize {
        self.rope.len_utf16_cu()
    }

    fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        if line.get() >= self.rope.len_lines() {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }

        Ok(CharOffset::new(self.rope.line_to_char(line.get())))
    }

    fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position> {
        let offset_value = offset.get();

        if offset_value > self.rope.len_chars() {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        if is_crlf_middle(&self.rope, offset_value) {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        let line_idx = self.rope.char_to_line(offset_value);
        let line_start = self.rope.line_to_char(line_idx);
        let column = offset_value - line_start;

        Ok(Position::new(
            Line::new(line_idx),
            LogicalColumn::new(column),
        ))
    }

    fn position_to_char(&self, position: Position) -> EngineResult<CharOffset> {
        let line = position.line();
        let column = position.column().get();

        if line.get() >= self.rope.len_lines() {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }

        let line_start = self.rope.line_to_char(line.get());
        let next_line_start = if line.get() + 1 < self.rope.len_lines() {
            self.rope.line_to_char(line.get() + 1)
        } else {
            self.rope.len_chars()
        };

        let line_content_end = line_content_end(&self.rope, next_line_start);
        let line_len = line_content_end - line_start;

        if column <= line_len {
            return Ok(CharOffset::new(line_start + column));
        }

        Err(CoordinateError::OutOfBounds(CharOffset::new(line_content_end)).into())
    }

    fn byte_to_char(&self, offset: ByteOffset) -> EngineResult<CharOffset> {
        byte_to_char_in_text(&self.rope.to_string(), offset)
    }

    fn char_to_byte(&self, offset: CharOffset) -> EngineResult<ByteOffset> {
        char_to_byte_in_text(&self.rope.to_string(), offset)
    }

    fn char_to_utf16_position(&self, offset: CharOffset) -> EngineResult<Utf16Position> {
        char_to_utf16_position_in_rope(&self.rope, offset)
    }

    fn utf16_position_to_char(&self, position: Utf16Position) -> EngineResult<CharOffset> {
        utf16_position_to_char_in_rope(&self.rope, position)
    }

    fn is_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<bool> {
        is_grapheme_boundary_in_text(&self.rope.to_string(), offset)
    }

    fn previous_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        previous_grapheme_boundary_in_text(&self.rope.to_string(), offset)
    }

    fn next_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        next_grapheme_boundary_in_text(&self.rope.to_string(), offset)
    }

    fn line_ending_style(&self) -> LineEndingStyle {
        detect_line_ending_style(&self.rope.to_string())
    }

    fn char_at(&self, offset: CharOffset) -> Option<char> {
        char_at(&self.rope, offset.get())
    }
}

impl TextStorage for RopeyStorage {
    type Snapshot = RopeySnapshot;

    fn snapshot(&self) -> Self::Snapshot {
        RopeySnapshot {
            rope: self.rope.clone(),
        }
    }

    fn replace(&mut self, range: TextRange, replacement: &str) -> EngineResult<()> {
        self.validate_range(range)?;

        let start = range.start().get();
        let end = range.end().get();

        if start != end {
            self.rope.remove(start..end);
        }

        if !replacement.is_empty() {
            self.rope.insert(start, replacement);
        }

        Ok(())
    }
}

/// Ropey-backed 不可变快照。
///
/// `Rope::clone()` 是共享底层数据的低成本 clone，因此 Snapshot 不再需要复制整篇文本。
#[derive(Debug, Clone)]
pub(crate) struct RopeySnapshot {
    rope: Rope,
}

impl TextRead for RopeySnapshot {
    fn text(&self) -> Cow<'_, str> {
        Cow::Owned(self.rope.to_string())
    }

    fn slice_text(&self, range: TextRange) -> EngineResult<Cow<'_, str>> {
        if range.end().get() > self.rope.len_chars() {
            return Err(EditError::RangeOutOfBounds { range }.into());
        }

        Ok(Cow::Owned(
            self.rope
                .slice(range.start().get()..range.end().get())
                .to_string(),
        ))
    }

    fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    fn len_chars(&self) -> CharOffset {
        CharOffset::new(self.rope.len_chars())
    }

    fn len_utf16_cu(&self) -> usize {
        self.rope.len_utf16_cu()
    }

    fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        if line.get() >= self.rope.len_lines() {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }

        Ok(CharOffset::new(self.rope.line_to_char(line.get())))
    }

    fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position> {
        RopeyStorage {
            rope: self.rope.clone(),
        }
        .char_to_position(offset)
    }

    fn position_to_char(&self, position: Position) -> EngineResult<CharOffset> {
        RopeyStorage {
            rope: self.rope.clone(),
        }
        .position_to_char(position)
    }

    fn byte_to_char(&self, offset: ByteOffset) -> EngineResult<CharOffset> {
        byte_to_char_in_text(&self.rope.to_string(), offset)
    }

    fn char_to_byte(&self, offset: CharOffset) -> EngineResult<ByteOffset> {
        char_to_byte_in_text(&self.rope.to_string(), offset)
    }

    fn char_to_utf16_position(&self, offset: CharOffset) -> EngineResult<Utf16Position> {
        char_to_utf16_position_in_rope(&self.rope, offset)
    }

    fn utf16_position_to_char(&self, position: Utf16Position) -> EngineResult<CharOffset> {
        utf16_position_to_char_in_rope(&self.rope, position)
    }

    fn is_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<bool> {
        is_grapheme_boundary_in_text(&self.rope.to_string(), offset)
    }

    fn previous_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        previous_grapheme_boundary_in_text(&self.rope.to_string(), offset)
    }

    fn next_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        next_grapheme_boundary_in_text(&self.rope.to_string(), offset)
    }

    fn line_ending_style(&self) -> LineEndingStyle {
        detect_line_ending_style(&self.rope.to_string())
    }

    fn char_at(&self, offset: CharOffset) -> Option<char> {
        char_at(&self.rope, offset.get())
    }
}

fn char_to_utf16_position_in_rope(rope: &Rope, offset: CharOffset) -> EngineResult<Utf16Position> {
    let offset_value = offset.get();

    if offset_value > rope.len_chars() {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    if is_crlf_middle(rope, offset_value) {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    let line_idx = rope.char_to_line(offset_value);
    let line_start = rope.line_to_char(line_idx);
    let utf16_units = rope
        .slice(line_start..offset_value)
        .to_string()
        .encode_utf16()
        .count();

    Ok(Utf16Position::new(
        Line::new(line_idx),
        Utf16Offset::new(utf16_units),
    ))
}

fn utf16_position_to_char_in_rope(
    rope: &Rope,
    position: Utf16Position,
) -> EngineResult<CharOffset> {
    let line = position.line();

    if line.get() >= rope.len_lines() {
        return Err(CoordinateError::LineOutOfBounds(line).into());
    }

    let line_start = rope.line_to_char(line.get());
    let next_line_start = if line.get() + 1 < rope.len_lines() {
        rope.line_to_char(line.get() + 1)
    } else {
        rope.len_chars()
    };
    let line_content_end = line_content_end(rope, next_line_start);
    let line_text = rope.slice(line_start..line_content_end).to_string();
    let target = position.character().get();

    let mut utf16_units = 0usize;
    let mut char_count = 0usize;

    if target == 0 {
        return Ok(CharOffset::new(line_start));
    }

    for ch in line_text.chars() {
        let next_utf16_units = utf16_units + ch.len_utf16();
        let next_char_count = char_count + 1;

        if target == next_utf16_units {
            return Ok(CharOffset::new(line_start + next_char_count));
        }

        if target < next_utf16_units {
            return Err(CoordinateError::InvalidUtf16Boundary(position).into());
        }

        utf16_units = next_utf16_units;
        char_count = next_char_count;
    }

    if target == utf16_units {
        return Ok(CharOffset::new(line_start + char_count));
    }

    Err(CoordinateError::Utf16PositionOutOfBounds(position).into())
}

fn char_to_byte_in_text(text: &str, offset: CharOffset) -> EngineResult<ByteOffset> {
    let char_offset = offset.get();
    let len_chars = text.chars().count();

    if char_offset > len_chars {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    if char_offset == len_chars {
        return Ok(ByteOffset::new(text.len()));
    }

    text.char_indices()
        .nth(char_offset)
        .map(|(byte_idx, _)| ByteOffset::new(byte_idx))
        .ok_or_else(|| CoordinateError::OutOfBounds(offset).into())
}

fn byte_to_char_in_text(text: &str, offset: ByteOffset) -> EngineResult<CharOffset> {
    let byte_offset = offset.get();

    if byte_offset > text.len() {
        return Err(CoordinateError::ByteOutOfBounds(offset).into());
    }

    if !text.is_char_boundary(byte_offset) {
        return Err(CoordinateError::InvalidByteBoundary(offset).into());
    }

    Ok(CharOffset::new(text[..byte_offset].chars().count()))
}

fn is_grapheme_boundary_in_text(text: &str, offset: CharOffset) -> EngineResult<bool> {
    let byte_offset = char_to_byte_in_text(text, offset)?.get();

    if byte_offset == 0 || byte_offset == text.len() {
        return Ok(true);
    }

    Ok(UnicodeSegmentation::grapheme_indices(text, true)
        .any(|(boundary, _)| boundary == byte_offset))
}

fn previous_grapheme_boundary_in_text(text: &str, offset: CharOffset) -> EngineResult<CharOffset> {
    let byte_offset = char_to_byte_in_text(text, offset)?.get();
    let mut previous = 0usize;

    for (boundary, _) in UnicodeSegmentation::grapheme_indices(text, true) {
        if boundary >= byte_offset {
            break;
        }
        previous = boundary;
    }

    byte_to_char_in_text(text, ByteOffset::new(previous))
}

fn next_grapheme_boundary_in_text(text: &str, offset: CharOffset) -> EngineResult<CharOffset> {
    let byte_offset = char_to_byte_in_text(text, offset)?.get();

    for (boundary, _) in UnicodeSegmentation::grapheme_indices(text, true) {
        if boundary > byte_offset {
            return byte_to_char_in_text(text, ByteOffset::new(boundary));
        }
    }

    byte_to_char_in_text(text, ByteOffset::new(text.len()))
}

fn detect_line_ending_style(text: &str) -> LineEndingStyle {
    let bytes = text.as_bytes();
    let mut has_lf = false;
    let mut has_crlf = false;
    let mut has_lone_cr = false;
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b'\r' if i + 1 < bytes.len() && bytes[i + 1] == b'\n' => {
                has_crlf = true;
                i += 2;
            }
            b'\r' => {
                has_lone_cr = true;
                i += 1;
            }
            b'\n' => {
                has_lf = true;
                i += 1;
            }
            _ => i += 1,
        }
    }

    match (has_lf, has_crlf, has_lone_cr) {
        (false, false, false) => LineEndingStyle::None,
        (true, false, false) => LineEndingStyle::Lf,
        (false, true, false) => LineEndingStyle::Crlf,
        _ => LineEndingStyle::Mixed,
    }
}

fn line_content_end(rope: &Rope, next_line_start: usize) -> usize {
    if next_line_start == 0 {
        return next_line_start;
    }

    let Some(prev) = char_at(rope, next_line_start - 1) else {
        return next_line_start;
    };

    if prev != '\n' {
        return next_line_start;
    }

    let without_lf = next_line_start - 1;

    if without_lf > 0 && char_at(rope, without_lf - 1) == Some('\r') {
        without_lf - 1
    } else {
        without_lf
    }
}

fn is_crlf_middle(rope: &Rope, offset: usize) -> bool {
    offset > 0
        && offset < rope.len_chars()
        && char_at(rope, offset - 1) == Some('\r')
        && char_at(rope, offset) == Some('\n')
}

fn char_at(rope: &Rope, char_offset: usize) -> Option<char> {
    if char_offset >= rope.len_chars() {
        return None;
    }

    Some(rope.char(char_offset))
}
