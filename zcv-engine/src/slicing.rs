//! 文本切片 public 类型与边界数学。
//!
//! 只表达只读文本读取接口，不表达折叠投影或渲染坐标。

use std::{borrow::Cow, fmt};

use crate::{
    ByteOffset, CoordinateError, EngineResult, Line, LineRange, TextRange, storage::TextRead,
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
/// 对应逻辑行在全文中的精确 byte 范围；行以换行符结束时切片保留换行符。
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

/// 单行文本内容（剥掉行尾换行符）。
///
/// 整行范围包含行尾换行符；内容本身去掉行尾换行符，并按 `max_line_chars` 截断（`None` 表示不截断）。
/// 供软换行片段切分等读取行内容的场景使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineContent<'a> {
    line: Line,
    full_range: TextRange,
    text: TextSlice<'a>,
}

impl<'a> LineContent<'a> {
    pub(crate) fn new(line: Line, full_range: TextRange, text: TextSlice<'a>) -> Self {
        Self {
            line,
            full_range,
            text,
        }
    }

    pub fn line(&self) -> Line {
        self.line
    }

    pub fn full_range(&self) -> TextRange {
        self.full_range
    }

    pub fn text_range(&self) -> TextRange {
        self.text.range()
    }

    pub fn as_str(&self) -> &str {
        self.text.as_str()
    }

    pub fn len_chars(&self) -> usize {
        self.text.len_chars()
    }

    pub fn len_bytes(&self) -> usize {
        self.text.len_bytes()
    }
}

impl AsRef<str> for LineContent<'_> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for LineContent<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub(crate) fn line_content_for_text<T: TextRead>(
    text: &T,
    line: Line,
    max_line_chars: Option<usize>,
) -> EngineResult<LineContent<'_>> {
    let full_range = text_range_for_line(text, line)?;
    let content_range = content_range_for_line(text, full_range, max_line_chars)?;
    let content = TextSlice::new(content_range, text.slice_text(content_range)?);

    Ok(LineContent::new(line, full_range, content))
}

fn content_range_for_line<T: TextRead>(
    text: &T,
    full_range: TextRange,
    max_line_chars: Option<usize>,
) -> EngineResult<TextRange> {
    let content_end = line_content_end(text, full_range)?;
    let end = match max_line_chars {
        Some(max_line_chars) => {
            byte_offset_after_chars(text, full_range.start(), content_end, max_line_chars)?
        }
        None => content_end,
    };

    Ok(TextRange::new(full_range.start(), end)?)
}

fn byte_offset_after_chars<T: TextRead>(
    text: &T,
    start: ByteOffset,
    end: ByteOffset,
    max_chars: usize,
) -> EngineResult<ByteOffset> {
    if max_chars == 0 {
        return Ok(start);
    }

    let range = TextRange::new(start, end)?;
    let mut cursor = start.get();
    let mut remaining = max_chars;

    for chunk in text.chunks(range)? {
        for (byte, _) in chunk.char_indices() {
            if remaining == 0 {
                return Ok(ByteOffset::new(cursor + byte));
            }
            remaining -= 1;
        }

        cursor += chunk.len();
        if remaining == 0 {
            return Ok(ByteOffset::new(cursor));
        }
    }

    Ok(end)
}

fn line_content_end<T: TextRead>(text: &T, full_range: TextRange) -> EngineResult<ByteOffset> {
    let start_value = full_range.start().get();
    let end = full_range.end();
    let end_value = end.get();

    if end_value == start_value {
        return Ok(end);
    }

    // 用 byte 接口判断结尾的 \n / \r\n。
    if text.char_at_byte(ByteOffset::new(end_value - 1)) != Some('\n') {
        return Ok(end);
    }

    let without_lf = end_value - 1;

    if without_lf > start_value && text.char_at_byte(ByteOffset::new(without_lf - 1)) == Some('\r')
    {
        Ok(ByteOffset::new(without_lf - 1))
    } else {
        Ok(ByteOffset::new(without_lf))
    }
}

pub(crate) fn text_range_for_byte_range<T: TextRead>(
    _text: &T,
    start: ByteOffset,
    end: ByteOffset,
) -> EngineResult<TextRange> {
    if start > end {
        return Err(CoordinateError::InvalidRange { start, end }.into());
    }

    // TextRange 内核就是字节区间；端点是否落在字符边界由 Storage 在 slice/replace 时校验。
    Ok(TextRange::new(start, end)?)
}

pub(crate) fn text_range_for_line<T: TextRead>(text: &T, line: Line) -> EngineResult<TextRange> {
    let start = text.line_start(line)?;
    let next_line = line
        .get()
        .checked_add(1)
        .map(Line::new)
        .ok_or(CoordinateError::LineOutOfBounds(line))?;
    let end = byte_offset_for_line_boundary(text, next_line)?;

    Ok(TextRange::new(start, end)?)
}

pub(crate) fn text_range_for_line_range<T: TextRead>(
    text: &T,
    line_range: LineRange,
) -> EngineResult<TextRange> {
    let start = byte_offset_for_line_boundary(text, line_range.start())?;
    let end = byte_offset_for_line_boundary(text, line_range.end())?;
    Ok(TextRange::new(start, end)?)
}

fn byte_offset_for_line_boundary<T: TextRead>(text: &T, line: Line) -> EngineResult<ByteOffset> {
    let line_value = line.get();
    let line_count = text.line_count();

    if line_value > line_count {
        return Err(CoordinateError::LineOutOfBounds(line).into());
    }

    if line_value == line_count {
        return Ok(text.len_bytes());
    }

    text.line_start(line)
}
