use std::borrow::Cow;

use crate::{
    BufferConfig, BufferVersion, ByteOffset, CharOffset, CoordinateError, DisplayColumn,
    DisplayColumnAffinity, EngineResult, Line, LineEndingStyle, LogicalColumn, Position,
    Utf16Position,
    storage::{RopeySnapshot, TextRead},
};

use crate::buffer::{
    Buffer,
    coordinates::{
        char_to_display_column_in_text, display_to_logical_column_in_text,
        logical_to_display_column_in_text, next_tab_stop,
    },
};

/// 不可变文本快照。
#[derive(Debug, Clone)]
pub struct Snapshot {
    storage: RopeySnapshot,
    version: BufferVersion,
    config: BufferConfig,
}

impl Snapshot {
    pub(crate) fn new(
        storage: RopeySnapshot,
        version: BufferVersion,
        config: BufferConfig,
    ) -> Self {
        Self {
            storage,
            version,
            config,
        }
    }

    pub fn text(&self) -> Cow<'_, str> {
        self.storage.text()
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    pub fn len_chars(&self) -> CharOffset {
        self.storage.len_chars()
    }

    pub fn len_bytes(&self) -> usize {
        self.storage.len_bytes()
    }

    pub fn len_utf16_cu(&self) -> usize {
        self.storage.len_utf16_cu()
    }

    pub fn line_count(&self) -> usize {
        self.storage.line_count()
    }

    pub fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        self.storage.line_start(line)
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
        self.storage.is_grapheme_boundary(offset)
    }

    pub fn validate_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<()> {
        if self.storage.is_grapheme_boundary(offset)? {
            Ok(())
        } else {
            Err(CoordinateError::InvalidGraphemeBoundary(offset).into())
        }
    }

    pub fn previous_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.storage.previous_grapheme_boundary(offset)
    }

    pub fn next_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.storage.next_grapheme_boundary(offset)
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

    pub fn is_stale_for(&self, buffer: &Buffer) -> bool {
        self.version != buffer.version()
    }
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version && self.text() == other.text()
    }
}

impl Eq for Snapshot {}
