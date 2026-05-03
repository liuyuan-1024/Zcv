//! 最小可编辑 Buffer。
//!
//! M1 目标：
//! - Buffer 创建与读取
//! - insert / delete / replace
//! - TextRange 校验
//! - 基础 LineIndex
//! - ByteOffset <-> Position
//! - BufferVersion 递增
//! - dirty state

mod line_index;

use crate::storage::{StringStorage, TextStorage};
use crate::{
    BufferConfig, BufferVersion, ByteOffset, CoordinateError, EditError, EngineError, EngineResult,
    Line, Position, TextRange,
};

use line_index::LineIndex;

/// 最小可编辑 Buffer。
#[derive(Debug, Clone)]
pub struct Buffer {
    config: BufferConfig,
    storage: StringStorage,
    version: BufferVersion,
    saved_version: BufferVersion,
    line_index: LineIndex,
}

impl Buffer {
    /// 创建空 Buffer。
    pub fn new(config: BufferConfig) -> EngineResult<Self> {
        Self::from_text(String::new(), config)
    }

    /// 从已有文本创建 Buffer。
    pub fn from_text(text: String, config: BufferConfig) -> EngineResult<Self> {
        let line_index = LineIndex::build(&text);

        Ok(Self {
            config,
            storage: StringStorage::new(text),
            version: BufferVersion::INITIAL,
            saved_version: BufferVersion::INITIAL,
            line_index,
        })
    }

    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    pub fn text(&self) -> &str {
        self.storage.text()
    }

    pub fn len_bytes(&self) -> ByteOffset {
        self.storage.len_bytes()
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn saved_version(&self) -> BufferVersion {
        self.saved_version
    }

    pub fn is_dirty(&self) -> bool {
        self.version != self.saved_version
    }

    pub fn mark_saved(&mut self) {
        self.saved_version = self.version;
    }

    pub fn line_count(&self) -> usize {
        self.line_index.line_count()
    }

    pub fn line_start(&self, line: Line) -> EngineResult<ByteOffset> {
        self.line_index.line_start(line)
    }

    pub fn byte_to_position(&self, offset: ByteOffset) -> EngineResult<Position> {
        self.line_index.byte_to_position(self.text(), offset)
    }

    pub fn position_to_byte(&self, position: Position) -> EngineResult<ByteOffset> {
        self.line_index.position_to_byte(self.text(), position)
    }

    pub fn insert(&mut self, offset: ByteOffset, text: &str) -> EngineResult<()> {
        let range = TextRange::new(offset, offset)?;
        self.replace(range, text)
    }

    pub fn delete(&mut self, range: TextRange) -> EngineResult<()> {
        self.replace(range, "")
    }

    /// 替换指定范围的文本，支持插入和删除。
    pub fn replace(&mut self, range: TextRange, replacement: &str) -> EngineResult<()> {
        self.validate_range(range)?;
        self.validate_edit_boundary(range.start())?;
        self.validate_edit_boundary(range.end())?;

        let start = range.start().get();
        let end = range.end().get();

        // no-op 不递增版本，也不污染 dirty。
        if &self.text()[start..end] == replacement {
            return Ok(());
        }

        self.storage.replace(range, replacement)?;
        self.line_index = LineIndex::build(self.text());
        self.bump_version()?;

        Ok(())
    }

    /// 校验范围是否合法，超出文本范围返回错误。
    fn validate_range(&self, range: TextRange) -> EngineResult<()> {
        if range.end().get() > self.text().len() {
            return Err(EditError::RangeOutOfBounds { range }.into());
        }

        Ok(())
    }

    /// 校验编辑边界是否合法，超出文本范围或非 UTF-8 边界返回错误。
    fn validate_edit_boundary(&self, offset: ByteOffset) -> EngineResult<()> {
        let value = offset.get();
        let text = self.text();

        if value > text.len() {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        if !text.is_char_boundary(value) {
            return Err(CoordinateError::InvalidUtf8Boundary(offset).into());
        }

        if is_crlf_middle(text, value) {
            return Err(EditError::InvalidBoundary { offset }.into());
        }

        Ok(())
    }

    /// 递增版本号，溢出时返回错误。
    fn bump_version(&mut self) -> EngineResult<()> {
        self.version = self.version.next().ok_or(EngineError::VersionOverflow)?;
        Ok(())
    }
}

fn is_crlf_middle(text: &str, offset: usize) -> bool {
    let bytes = text.as_bytes();

    offset > 0 && offset < bytes.len() && bytes[offset - 1] == b'\r' && bytes[offset] == b'\n'
}
