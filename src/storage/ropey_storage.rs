use std::borrow::Cow;

use ropey::Rope;

use super::{TextRead, TextStorage};
use crate::{
    CharOffset, CoordinateError, EditError, EngineResult, Line, LogicalColumn, Position, TextRange,
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

    fn char_at(&self, offset: CharOffset) -> Option<char> {
        char_at(&self.rope, offset.get())
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
