use crate::{ByteOffset, CoordinateError, EngineResult, Line, LogicalColumn, Position};

/// M1 基础行索引。
///
/// 语义：
/// - 空 Buffer 也有 1 行。
/// - 末尾换行会产生一个空的末行。
/// - LF / CRLF 都以 `\n` 作为下一行起点。
/// - M1 全量重建索引，M4 再做增量更新。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineIndex {
    starts: Vec<ByteOffset>,
}

impl LineIndex {
    pub(crate) fn build(text: &str) -> Self {
        let mut starts = vec![ByteOffset::ZERO];

        for (idx, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(ByteOffset::new(idx + 1));
            }
        }

        Self { starts }
    }

    pub(crate) fn line_count(&self) -> usize {
        self.starts.len()
    }

    pub(crate) fn line_start(&self, line: Line) -> EngineResult<ByteOffset> {
        self.starts
            .get(line.get())
            .copied()
            .ok_or_else(|| CoordinateError::LineOutOfBounds(line).into())
    }

    pub(crate) fn byte_to_position(
        &self,
        text: &str,
        offset: ByteOffset,
    ) -> EngineResult<Position> {
        let offset_value = offset.get();

        if offset_value > text.len() {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        if !text.is_char_boundary(offset_value) {
            return Err(CoordinateError::InvalidUtf8Boundary(offset).into());
        }

        if is_crlf_middle(text, offset_value) {
            return Err(CoordinateError::InvalidUtf8Boundary(offset).into());
        }

        let line_index = self
            .starts
            .partition_point(|start| start.get() <= offset_value)
            .saturating_sub(1);

        let line_start = self.starts[line_index].get();
        let column = text[line_start..offset_value].chars().count();

        Ok(Position::new(
            Line::new(line_index),
            LogicalColumn::new(column),
        ))
    }

    pub(crate) fn position_to_byte(
        &self,
        text: &str,
        position: Position,
    ) -> EngineResult<ByteOffset> {
        let line = position.line();
        let column = position.column().get();

        let line_start = self.line_start(line)?.get();

        let next_line_start = self
            .starts
            .get(line.get() + 1)
            .map(|offset| offset.get())
            .unwrap_or(text.len());

        let line_content_end = line_content_end(text, line_start, next_line_start);
        let line_text = &text[line_start..line_content_end];

        if column == 0 {
            return Ok(ByteOffset::new(line_start));
        }

        let mut current_column = 0;

        for (relative_byte, _) in line_text.char_indices() {
            if current_column == column {
                return Ok(ByteOffset::new(line_start + relative_byte));
            }

            current_column += 1;
        }

        if current_column == column {
            return Ok(ByteOffset::new(line_content_end));
        }

        Err(CoordinateError::OutOfBounds(ByteOffset::new(line_content_end)).into())
    }
}

fn line_content_end(text: &str, line_start: usize, next_line_start: usize) -> usize {
    let bytes = text.as_bytes();

    if next_line_start > line_start && bytes[next_line_start - 1] == b'\n' {
        if next_line_start >= line_start + 2 && bytes[next_line_start - 2] == b'\r' {
            next_line_start - 2
        } else {
            next_line_start - 1
        }
    } else {
        next_line_start
    }
}

fn is_crlf_middle(text: &str, offset: usize) -> bool {
    let bytes = text.as_bytes();

    offset > 0 && offset < bytes.len() && bytes[offset - 1] == b'\r' && bytes[offset] == b'\n'
}
