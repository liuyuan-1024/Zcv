//! 文本切片 public 类型与边界数学。
//!
//! M11A 只提供只读文本读取接口，不表达 viewport、折叠投影或渲染坐标。

use std::{borrow::Cow, fmt};

use crate::{
    ByteOffset, CharOffset, CoordinateError, EngineResult, Line, LineRange, TextRange,
    storage::TextRead,
};

/// 只读文本切片。
///
/// `TextSlice` 绑定原文中的 `TextRange`，文本内容按需由存储后端借用或拼接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSlice<'a> {
    range: TextRange,
    text: Cow<'a, str>,
}

impl<'a> TextSlice<'a> {
    pub(crate) fn new(range: TextRange, text: Cow<'a, str>) -> Self {
        Self { range, text }
    }

    pub fn range(&self) -> TextRange {
        self.range
    }

    pub fn as_str(&self) -> &str {
        self.text.as_ref()
    }

    pub fn into_text(self) -> Cow<'a, str> {
        self.text
    }

    pub fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    pub fn len_bytes(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

impl AsRef<str> for TextSlice<'_> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for TextSlice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 单行只读文本切片。
///
/// `range` 是该逻辑行在全文中的精确 char range；如果该行以换行符结束，切片会保留换行符。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSlice<'a> {
    line: Line,
    text: TextSlice<'a>,
}

impl<'a> LineSlice<'a> {
    pub(crate) fn new(line: Line, text: TextSlice<'a>) -> Self {
        Self { line, text }
    }

    pub fn line(&self) -> Line {
        self.line
    }

    pub fn range(&self) -> TextRange {
        self.text.range()
    }

    pub fn text_slice(&self) -> &TextSlice<'a> {
        &self.text
    }

    pub fn into_text_slice(self) -> TextSlice<'a> {
        self.text
    }

    pub fn as_str(&self) -> &str {
        self.text.as_str()
    }

    pub fn into_text(self) -> Cow<'a, str> {
        self.text.into_text()
    }

    pub fn len_chars(&self) -> usize {
        self.text.len_chars()
    }

    pub fn len_bytes(&self) -> usize {
        self.text.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

impl AsRef<str> for LineSlice<'_> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for LineSlice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub(crate) fn text_range_for_byte_range<T: TextRead>(
    text: &T,
    start: ByteOffset,
    end: ByteOffset,
) -> EngineResult<TextRange> {
    if start > end {
        return Err(CoordinateError::InvalidByteRange { start, end }.into());
    }

    Ok(TextRange::new(
        text.byte_to_char(start)?,
        text.byte_to_char(end)?,
    )?)
}

pub(crate) fn text_range_for_line<T: TextRead>(text: &T, line: Line) -> EngineResult<TextRange> {
    let start = text.line_start(line)?;
    let next_line = line
        .get()
        .checked_add(1)
        .map(Line::new)
        .ok_or(CoordinateError::LineOutOfBounds(line))?;
    let end = char_offset_for_line_boundary(text, next_line)?;

    Ok(TextRange::new(start, end)?)
}

pub(crate) fn text_range_for_line_range<T: TextRead>(
    text: &T,
    line_range: LineRange,
) -> EngineResult<TextRange> {
    let start = char_offset_for_line_boundary(text, line_range.start())?;
    let end = char_offset_for_line_boundary(text, line_range.end())?;
    Ok(TextRange::new(start, end)?)
}

fn char_offset_for_line_boundary<T: TextRead>(text: &T, line: Line) -> EngineResult<CharOffset> {
    let line_value = line.get();
    let line_count = text.line_count();

    if line_value > line_count {
        return Err(CoordinateError::LineOutOfBounds(line).into());
    }

    if line_value == line_count {
        return Ok(text.len_chars());
    }

    text.line_start(line)
}
