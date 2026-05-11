//! Buffer 坐标门面：把存储层 byte 深核接口投影为编辑器需要的边界（char / UTF-16 / line / DisplayColumn）API。
//!
//! 本文件绑定 BufferConfig 并处理 CRLF、grapheme、DisplayColumn 等策略，不直接修改文本或历史。

use crate::{
    ByteOffset, CharOffset, CoordinateError, DisplayColumn, DisplayColumnAffinity, EngineResult,
    Line, LineEndingStyle, LogicalColumn, Position, Utf16Position,
    coordinates::core::{
        char_to_display_column_in_text, display_to_logical_column_in_text,
        logical_to_display_column_in_text, next_tab_stop,
    },
    storage::TextRead,
};

use super::Buffer;

impl Buffer {
    pub fn line_count(&self) -> usize {
        self.storage.line_count()
    }

    // ============== 深核：byte 接口 ==============

    /// 指定行的起始 ByteOffset（深核接口）。
    pub fn line_start_byte(&self, line: Line) -> EngineResult<ByteOffset> {
        self.storage.line_start(line)
    }

    pub fn byte_to_position(&self, offset: ByteOffset) -> EngineResult<Position> {
        self.storage.byte_to_position(offset)
    }

    pub fn position_to_byte(&self, position: Position) -> EngineResult<ByteOffset> {
        self.storage.position_to_byte(position)
    }

    pub fn is_grapheme_boundary_byte(&self, offset: ByteOffset) -> EngineResult<bool> {
        self.storage.is_grapheme_boundary(offset)
    }

    pub fn previous_grapheme_boundary_byte(&self, offset: ByteOffset) -> EngineResult<ByteOffset> {
        self.storage.previous_grapheme_boundary(offset)
    }

    pub fn next_grapheme_boundary_byte(&self, offset: ByteOffset) -> EngineResult<ByteOffset> {
        self.storage.next_grapheme_boundary(offset)
    }

    // ============== 边界投影：CharOffset / Line/Column / UTF-16 ==============

    /// 指定行的起始 CharOffset（边界投影）。
    pub fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        let byte = self.storage.line_start(line)?;
        self.storage.byte_to_char(byte)
    }

    pub fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position> {
        self.storage.char_to_position(offset)
    }

    pub fn position_to_char(&self, position: Position) -> EngineResult<CharOffset> {
        self.storage.position_to_char(position)
    }

    pub fn byte_to_char(&self, offset: ByteOffset) -> EngineResult<CharOffset> {
        self.storage.byte_to_char(offset)
    }

    pub fn char_to_byte(&self, offset: CharOffset) -> EngineResult<ByteOffset> {
        self.storage.char_to_byte(offset)
    }

    pub fn char_to_utf16_position(&self, offset: CharOffset) -> EngineResult<Utf16Position> {
        self.storage.char_to_utf16_position(offset)
    }

    pub fn utf16_position_to_char(&self, position: Utf16Position) -> EngineResult<CharOffset> {
        self.storage.utf16_position_to_char(position)
    }

    pub fn byte_to_utf16_position(&self, offset: ByteOffset) -> EngineResult<Utf16Position> {
        self.storage.byte_to_utf16_position(offset)
    }

    pub fn utf16_position_to_byte(&self, position: Utf16Position) -> EngineResult<ByteOffset> {
        self.storage.utf16_position_to_byte(position)
    }

    pub fn is_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<bool> {
        self.storage.is_grapheme_boundary_char(offset)
    }

    pub fn validate_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<()> {
        if self.storage.is_grapheme_boundary_char(offset)? {
            Ok(())
        } else {
            let byte = self.storage.char_to_byte(offset)?;
            Err(CoordinateError::InvalidGraphemeBoundary(byte).into())
        }
    }

    pub fn previous_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.storage.previous_grapheme_boundary_char(offset)
    }

    pub fn next_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.storage.next_grapheme_boundary_char(offset)
    }

    pub fn line_ending_style(&self) -> LineEndingStyle {
        self.storage.line_ending_style()
    }

    pub fn next_tab_stop(&self, display_column: DisplayColumn) -> DisplayColumn {
        next_tab_stop(display_column, self.config.tab.tab_width())
    }

    pub fn char_to_display_column(&self, offset: CharOffset) -> EngineResult<DisplayColumn> {
        char_to_display_column_in_text(&self.storage, &self.config, offset)
    }

    pub fn logical_to_display_column(
        &self,
        line: Line,
        column: LogicalColumn,
    ) -> EngineResult<DisplayColumn> {
        logical_to_display_column_in_text(&self.storage, &self.config, line, column)
    }

    pub fn display_to_logical_column(
        &self,
        line: Line,
        column: DisplayColumn,
    ) -> EngineResult<LogicalColumn> {
        display_to_logical_column_in_text(
            &self.storage,
            &self.config,
            line,
            column,
            self.config.display_width.affinity,
        )
    }

    pub fn display_to_logical_column_with_affinity(
        &self,
        line: Line,
        column: DisplayColumn,
        affinity: DisplayColumnAffinity,
    ) -> EngineResult<LogicalColumn> {
        display_to_logical_column_in_text(&self.storage, &self.config, line, column, affinity)
    }

    pub fn display_column_to_char(
        &self,
        line: Line,
        column: DisplayColumn,
    ) -> EngineResult<CharOffset> {
        let logical = self.display_to_logical_column(line, column)?;
        self.storage.position_to_char(Position::new(line, logical))
    }

    pub fn display_column_to_char_with_affinity(
        &self,
        line: Line,
        column: DisplayColumn,
        affinity: DisplayColumnAffinity,
    ) -> EngineResult<CharOffset> {
        let logical = self.display_to_logical_column_with_affinity(line, column, affinity)?;
        self.storage.position_to_char(Position::new(line, logical))
    }
}

pub(super) fn char_to_byte_index(text: &str, offset: CharOffset) -> EngineResult<usize> {
    let char_offset = offset.get();
    let len_chars = text.chars().count();

    if char_offset > len_chars {
        return Err(CoordinateError::CharOutOfBounds(offset).into());
    }

    if char_offset == len_chars {
        return Ok(text.len());
    }

    text.char_indices()
        .nth(char_offset)
        .map(|(byte_idx, _)| byte_idx)
        .ok_or_else(|| CoordinateError::CharOutOfBounds(offset).into())
}

pub(super) fn is_crlf_middle<T: TextRead>(storage: &T, offset: CharOffset) -> bool {
    let value = offset.get();

    value > 0
        && value < storage.len_chars().get()
        && storage.char_at(CharOffset::new(value - 1)) == Some('\r')
        && storage.char_at(offset) == Some('\n')
}
