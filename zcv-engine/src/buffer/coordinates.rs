//! Buffer 坐标门面：把存储层 byte 深核接口投影为编辑器需要的边界（char / UTF-16 / line / DisplayColumn）API。
//!
//! 本文件绑定 BufferConfig 并处理 CRLF、grapheme、DisplayColumn 等策略，不直接修改文本或历史。

use crate::{
    ByteOffset, CharOffset, CoordinateError, EngineResult, Line, LineEndingStyle, Position,
    Utf16Offset, Utf16Position, storage::TextRead,
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

    /// `byte_to_position` 的省列变体。宿主 DisplayMap 的投影 / fold 几何只关心行号时，
    /// 走这条路径避免后端额外的 char/column 投影 O(log N)。
    pub fn byte_to_line(&self, offset: ByteOffset) -> EngineResult<Line> {
        self.storage.byte_to_line(offset)
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

    /// 全文 flat UTF-16 code unit 偏移：byte → utf16 cu。
    ///
    /// 给系统 IME（NSTextInputClient / TSF / IBus）的"扁平 UTF-16 offset"语义用，
    /// 不要走 `byte_to_utf16_position`（那是 LSP 协议的行/列）。
    pub fn byte_to_utf16_cu(&self, offset: ByteOffset) -> EngineResult<Utf16Offset> {
        self.storage.byte_to_utf16_cu(offset)
    }

    /// 全文 flat UTF-16 code unit 偏移：utf16 cu → byte。
    pub fn utf16_cu_to_byte(&self, offset: Utf16Offset) -> EngineResult<ByteOffset> {
        self.storage.utf16_cu_to_byte(offset)
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
}

pub(super) fn is_crlf_middle<T: TextRead>(storage: &T, offset: CharOffset) -> bool {
    let value = offset.get();

    value > 0
        && value < storage.len_chars().get()
        && storage.char_at(CharOffset::new(value - 1)) == Some('\r')
        && storage.char_at(offset) == Some('\n')
}
