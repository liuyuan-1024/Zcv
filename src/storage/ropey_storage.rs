use std::borrow::Cow;

use ropey::Rope;
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

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
        byte_to_char_in_rope(&self.rope, offset)
    }

    fn char_to_byte(&self, offset: CharOffset) -> EngineResult<ByteOffset> {
        char_to_byte_in_rope(&self.rope, offset)
    }

    fn char_to_utf16_position(&self, offset: CharOffset) -> EngineResult<Utf16Position> {
        char_to_utf16_position_in_rope(&self.rope, offset)
    }

    fn utf16_position_to_char(&self, position: Utf16Position) -> EngineResult<CharOffset> {
        utf16_position_to_char_in_rope(&self.rope, position)
    }

    fn is_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<bool> {
        is_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn previous_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        previous_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn next_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        next_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn line_ending_style(&self) -> LineEndingStyle {
        detect_line_ending_style_in_rope(&self.rope)
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
        byte_to_char_in_rope(&self.rope, offset)
    }

    fn char_to_byte(&self, offset: CharOffset) -> EngineResult<ByteOffset> {
        char_to_byte_in_rope(&self.rope, offset)
    }

    fn char_to_utf16_position(&self, offset: CharOffset) -> EngineResult<Utf16Position> {
        char_to_utf16_position_in_rope(&self.rope, offset)
    }

    fn utf16_position_to_char(&self, position: Utf16Position) -> EngineResult<CharOffset> {
        utf16_position_to_char_in_rope(&self.rope, position)
    }

    fn is_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<bool> {
        is_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn previous_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        previous_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn next_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        next_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn line_ending_style(&self) -> LineEndingStyle {
        detect_line_ending_style_in_rope(&self.rope)
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
        .chars()
        .map(char::len_utf16)
        .sum();

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
    let target = position.character().get();

    let mut utf16_units = 0usize;
    let mut char_count = 0usize;

    if target == 0 {
        return Ok(CharOffset::new(line_start));
    }

    for ch in rope.slice(line_start..line_content_end).chars() {
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

fn char_to_byte_in_rope(rope: &Rope, offset: CharOffset) -> EngineResult<ByteOffset> {
    let char_offset = offset.get();
    if char_offset > rope.len_chars() {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    Ok(ByteOffset::new(rope.char_to_byte(char_offset)))
}

fn byte_to_char_in_rope(rope: &Rope, offset: ByteOffset) -> EngineResult<CharOffset> {
    let byte_offset = offset.get();

    if byte_offset > rope.len_bytes() {
        return Err(CoordinateError::ByteOutOfBounds(offset).into());
    }

    if !is_utf8_char_boundary_in_rope(rope, byte_offset) {
        return Err(CoordinateError::InvalidByteBoundary(offset).into());
    }

    Ok(CharOffset::new(rope.byte_to_char(byte_offset)))
}

fn is_utf8_char_boundary_in_rope(rope: &Rope, byte_offset: usize) -> bool {
    if byte_offset == rope.len_bytes() {
        return true;
    }

    let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte_offset);
    chunk.is_char_boundary(byte_offset - chunk_start)
}

fn is_grapheme_boundary_in_rope(rope: &Rope, offset: CharOffset) -> EngineResult<bool> {
    let byte_offset = char_to_byte_in_rope(rope, offset)?.get();
    let mut cursor = GraphemeCursor::new(byte_offset, rope.len_bytes(), true);
    let (mut chunk, mut chunk_start, _, _) = rope.chunk_at_byte(byte_offset);

    loop {
        match cursor.is_boundary(chunk, chunk_start) {
            Ok(result) => return Ok(result),
            Err(GraphemeIncomplete::PreContext(context_offset)) => {
                let context_index = context_offset.saturating_sub(1);
                let (context_chunk, context_start, _, _) = rope.chunk_at_byte(context_index);
                cursor.provide_context(context_chunk, context_start);
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                if chunk_start == 0 {
                    return Ok(true);
                }

                let (prev_chunk, prev_start, _, _) = rope.chunk_at_byte(chunk_start - 1);
                chunk = prev_chunk;
                chunk_start = prev_start;
            }
            Err(GraphemeIncomplete::NextChunk) => {
                let next_start = chunk_start + chunk.len();
                if next_start >= rope.len_bytes() {
                    return Ok(true);
                }

                let (next_chunk, next_chunk_start, _, _) = rope.chunk_at_byte(next_start);
                chunk = next_chunk;
                chunk_start = next_chunk_start;
            }
            Err(GraphemeIncomplete::InvalidOffset) => {
                return Err(CoordinateError::InvalidGraphemeBoundary(offset).into());
            }
        }
    }
}

fn previous_grapheme_boundary_in_rope(rope: &Rope, offset: CharOffset) -> EngineResult<CharOffset> {
    let byte_offset = char_to_byte_in_rope(rope, offset)?.get();
    let mut cursor = GraphemeCursor::new(byte_offset, rope.len_bytes(), true);
    let (mut chunk, mut chunk_start, _, _) = rope.chunk_at_byte(byte_offset);

    loop {
        match cursor.prev_boundary(chunk, chunk_start) {
            Ok(Some(boundary)) => return Ok(CharOffset::new(rope.byte_to_char(boundary))),
            Ok(None) => return Ok(CharOffset::ZERO),
            Err(GraphemeIncomplete::PreContext(context_offset)) => {
                let context_index = context_offset.saturating_sub(1);
                let (context_chunk, context_start, _, _) = rope.chunk_at_byte(context_index);
                cursor.provide_context(context_chunk, context_start);
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                if chunk_start == 0 {
                    return Ok(CharOffset::ZERO);
                }

                let (prev_chunk, prev_start, _, _) = rope.chunk_at_byte(chunk_start - 1);
                chunk = prev_chunk;
                chunk_start = prev_start;
            }
            Err(GraphemeIncomplete::NextChunk) => {
                let next_start = chunk_start + chunk.len();
                if next_start >= rope.len_bytes() {
                    return Ok(CharOffset::new(rope.len_chars()));
                }

                let (next_chunk, next_chunk_start, _, _) = rope.chunk_at_byte(next_start);
                chunk = next_chunk;
                chunk_start = next_chunk_start;
            }
            Err(GraphemeIncomplete::InvalidOffset) => {
                return Err(CoordinateError::InvalidGraphemeBoundary(offset).into());
            }
        }
    }
}

fn next_grapheme_boundary_in_rope(rope: &Rope, offset: CharOffset) -> EngineResult<CharOffset> {
    let byte_offset = char_to_byte_in_rope(rope, offset)?.get();
    let mut cursor = GraphemeCursor::new(byte_offset, rope.len_bytes(), true);
    let (mut chunk, mut chunk_start, _, _) = rope.chunk_at_byte(byte_offset);

    loop {
        match cursor.next_boundary(chunk, chunk_start) {
            Ok(Some(boundary)) => return Ok(CharOffset::new(rope.byte_to_char(boundary))),
            Ok(None) => return Ok(CharOffset::new(rope.len_chars())),
            Err(GraphemeIncomplete::PreContext(context_offset)) => {
                let context_index = context_offset.saturating_sub(1);
                let (context_chunk, context_start, _, _) = rope.chunk_at_byte(context_index);
                cursor.provide_context(context_chunk, context_start);
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                if chunk_start == 0 {
                    return Ok(CharOffset::ZERO);
                }

                let (prev_chunk, prev_start, _, _) = rope.chunk_at_byte(chunk_start - 1);
                chunk = prev_chunk;
                chunk_start = prev_start;
            }
            Err(GraphemeIncomplete::NextChunk) => {
                let next_start = chunk_start + chunk.len();
                if next_start >= rope.len_bytes() {
                    return Ok(CharOffset::new(rope.len_chars()));
                }

                let (next_chunk, next_chunk_start, _, _) = rope.chunk_at_byte(next_start);
                chunk = next_chunk;
                chunk_start = next_chunk_start;
            }
            Err(GraphemeIncomplete::InvalidOffset) => {
                return Err(CoordinateError::InvalidGraphemeBoundary(offset).into());
            }
        }
    }
}

fn detect_line_ending_style_in_rope(rope: &Rope) -> LineEndingStyle {
    let mut has_lf = false;
    let mut has_crlf = false;
    let mut has_lone_cr = false;
    let mut pending_cr = false;

    for chunk in rope.chunks() {
        for byte in chunk.as_bytes() {
            match *byte {
                b'\n' if pending_cr => {
                    has_crlf = true;
                    pending_cr = false;
                }
                b'\n' => {
                    has_lf = true;
                }
                b'\r' => {
                    if pending_cr {
                        has_lone_cr = true;
                    }
                    pending_cr = true;
                }
                _ => {
                    if pending_cr {
                        has_lone_cr = true;
                        pending_cr = false;
                    }
                }
            }
        }
    }

    if pending_cr {
        has_lone_cr = true;
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
