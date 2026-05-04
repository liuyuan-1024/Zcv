use crate::{CharOffset, CoordinateError, EngineResult, Line, LogicalColumn, Position};

/// M1/M3.5 基础行索引。
///
/// 语义：
/// - 空 Buffer 也有 1 行。
/// - 末尾换行会产生一个空的末行。
/// - LF / CRLF 都以 `\n` 作为下一行起点。
/// - M3.5 起，行起点保存 CharOffset，而不是 ByteOffset。
/// - M4 接入 ropey 后，可逐步替换为 ropey 的 line API 或增量 cache。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineIndex {
    starts: Vec<CharOffset>,
}

impl LineIndex {
    pub(crate) fn build(text: &str) -> Self {
        let mut starts = vec![CharOffset::ZERO];

        for (char_idx, ch) in text.chars().enumerate() {
            if ch == '\n' {
                starts.push(CharOffset::new(char_idx + 1));
            }
        }

        Self { starts }
    }

    pub(crate) fn line_count(&self) -> usize {
        self.starts.len()
    }

    pub(crate) fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        self.starts
            .get(line.get())
            .copied()
            .ok_or_else(|| CoordinateError::LineOutOfBounds(line).into())
    }

    pub(crate) fn char_to_position(
        &self,
        text: &str,
        offset: CharOffset,
    ) -> EngineResult<Position> {
        let offset_value = offset.get();
        let text_len = text.chars().count();

        if offset_value > text_len {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        if is_crlf_middle(text, offset_value) {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        let line_index = self
            .starts
            .partition_point(|start| start.get() <= offset_value)
            .saturating_sub(1);

        let line_start = self.starts[line_index].get();
        let column = offset_value - line_start;

        Ok(Position::new(
            Line::new(line_index),
            LogicalColumn::new(column),
        ))
    }

    pub(crate) fn position_to_char(
        &self,
        text: &str,
        position: Position,
    ) -> EngineResult<CharOffset> {
        let line = position.line();
        let column = position.column().get();

        let line_start = self.line_start(line)?.get();

        let next_line_start = self
            .starts
            .get(line.get() + 1)
            .map(|offset| offset.get())
            .unwrap_or_else(|| text.chars().count());

        let line_content_end = line_content_end(text, line_start, next_line_start);
        let line_len = line_content_end - line_start;

        if column <= line_len {
            return Ok(CharOffset::new(line_start + column));
        }

        Err(CoordinateError::OutOfBounds(CharOffset::new(line_content_end)).into())
    }
}

fn line_content_end(text: &str, line_start: usize, next_line_start: usize) -> usize {
    let _ = line_start;

    if next_line_start == 0 {
        return next_line_start;
    }

    let Some(prev) = char_at(text, next_line_start - 1) else {
        return next_line_start;
    };

    if prev != '\n' {
        return next_line_start;
    }

    let without_lf = next_line_start - 1;

    if without_lf > 0 && char_at(text, without_lf - 1) == Some('\r') {
        without_lf - 1
    } else {
        without_lf
    }
}

fn is_crlf_middle(text: &str, offset: usize) -> bool {
    offset > 0
        && offset < text.chars().count()
        && char_at(text, offset - 1) == Some('\r')
        && char_at(text, offset) == Some('\n')
}

fn char_at(text: &str, char_offset: usize) -> Option<char> {
    text.chars().nth(char_offset)
}
