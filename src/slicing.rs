//! 文本切片 public 类型与边界数学。
//!
//! 只表达只读文本读取接口，不表达折叠投影或渲染坐标。

use std::{borrow::Cow, fmt};

use crate::{
    ByteOffset, CoordinateError, EngineResult, Line, LineRange, TextRange, storage::TextRead,
};

/// 逻辑行视口。
///
/// `Viewport` 只表达按逻辑行读取的文本窗口，不包含像素滚动、字体测量或折叠投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Viewport {
    start_line: Line,
    line_count: usize,
    max_line_chars: Option<usize>,
}

impl Viewport {
    pub const fn new(start_line: Line, line_count: usize) -> Self {
        Self {
            start_line,
            line_count,
            max_line_chars: None,
        }
    }

    /// 设置单行最大读取 char 数，用于避免 viewport 读取被超长行拖成整行读取。
    pub const fn with_max_line_chars(mut self, max_line_chars: usize) -> Self {
        self.max_line_chars = Some(max_line_chars);
        self
    }

    pub const fn without_line_limit(mut self) -> Self {
        self.max_line_chars = None;
        self
    }

    pub const fn start_line(self) -> Line {
        self.start_line
    }

    pub const fn line_count(self) -> usize {
        self.line_count
    }

    pub const fn max_line_chars(self) -> Option<usize> {
        self.max_line_chars
    }
}

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
/// `range` 是该逻辑行在全文中的精确 byte range；如果该行以换行符结束，切片会保留换行符。
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

/// viewport 中的一条可见逻辑行。
///
/// `full_range` 是整行范围，包含行尾换行符；`visible_range` 是实际返回的可见文本范围，
/// 会去掉行尾换行符，并按 `Viewport::max_line_chars` 截断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleLine<'a> {
    line: Line,
    full_range: TextRange,
    full_len_chars: usize,
    visible_text: TextSlice<'a>,
    is_truncated: bool,
}

impl<'a> VisibleLine<'a> {
    pub(crate) fn new(
        line: Line,
        full_range: TextRange,
        full_len_chars: usize,
        visible_text: TextSlice<'a>,
        is_truncated: bool,
    ) -> Self {
        Self {
            line,
            full_range,
            full_len_chars,
            visible_text,
            is_truncated,
        }
    }

    pub fn line(&self) -> Line {
        self.line
    }

    pub fn full_range(&self) -> TextRange {
        self.full_range
    }

    pub fn visible_range(&self) -> TextRange {
        self.visible_text.range()
    }

    pub fn text_slice(&self) -> &TextSlice<'a> {
        &self.visible_text
    }

    pub fn into_text_slice(self) -> TextSlice<'a> {
        self.visible_text
    }

    pub fn as_str(&self) -> &str {
        self.visible_text.as_str()
    }

    pub fn visible_len_chars(&self) -> usize {
        self.visible_text.len_chars()
    }

    pub fn visible_len_bytes(&self) -> usize {
        self.visible_text.len_bytes()
    }

    pub fn full_len_chars(&self) -> usize {
        self.full_len_chars
    }

    pub fn full_len_bytes(&self) -> usize {
        self.full_range.len()
    }

    pub fn is_truncated(&self) -> bool {
        self.is_truncated
    }
}

impl AsRef<str> for VisibleLine<'_> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for VisibleLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一次 viewport 读取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportSlice<'a> {
    viewport: Viewport,
    line_range: LineRange,
    lines: Vec<VisibleLine<'a>>,
}

impl<'a> ViewportSlice<'a> {
    pub(crate) fn new(
        viewport: Viewport,
        line_range: LineRange,
        lines: Vec<VisibleLine<'a>>,
    ) -> Self {
        Self {
            viewport,
            line_range,
            lines,
        }
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn line_range(&self) -> LineRange {
        self.line_range
    }

    pub fn lines(&self) -> &[VisibleLine<'a>] {
        &self.lines
    }

    pub fn into_lines(self) -> Vec<VisibleLine<'a>> {
        self.lines
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
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

pub(crate) fn viewport_slice_for_text<T: TextRead>(
    text: &T,
    viewport: Viewport,
) -> EngineResult<ViewportSlice<'_>> {
    let line_range = line_range_for_viewport(text, viewport)?;
    let mut lines = Vec::with_capacity(line_range.len());

    for line_value in line_range.start().get()..line_range.end().get() {
        let line = Line::new(line_value);
        lines.push(visible_line_for_text(
            text,
            line,
            viewport.max_line_chars(),
        )?);
    }

    Ok(ViewportSlice::new(viewport, line_range, lines))
}

pub(crate) fn visible_line_for_text<T: TextRead>(
    text: &T,
    line: Line,
    max_line_chars: Option<usize>,
) -> EngineResult<VisibleLine<'_>> {
    let full_range = text_range_for_line(text, line)?;
    let visible_range = visible_range_for_line(text, full_range, max_line_chars)?;
    let is_truncated = visible_range.end() < line_content_end(text, full_range)?;
    let full_len_chars = text_range_len_chars(text, full_range)?;
    let visible_text = TextSlice::new(visible_range, text.slice_text(visible_range)?);

    Ok(VisibleLine::new(
        line,
        full_range,
        full_len_chars,
        visible_text,
        is_truncated,
    ))
}

fn text_range_len_chars<T: TextRead>(text: &T, range: TextRange) -> EngineResult<usize> {
    let mut len = 0;
    for chunk in text.chunks(range)? {
        len += chunk.chars().count();
    }
    Ok(len)
}

fn line_range_for_viewport<T: TextRead>(text: &T, viewport: Viewport) -> EngineResult<LineRange> {
    let start = viewport.start_line();
    let line_count = text.line_count();

    if start.get() > line_count {
        return Err(CoordinateError::LineOutOfBounds(start).into());
    }

    let end = start
        .get()
        .saturating_add(viewport.line_count())
        .min(line_count);
    Ok(LineRange::new(start, Line::new(end))?)
}

fn visible_range_for_line<T: TextRead>(
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
        let chunk_chars = chunk.chars().count();
        if remaining >= chunk_chars {
            cursor += chunk.len();
            remaining -= chunk_chars;
            if remaining == 0 {
                return Ok(ByteOffset::new(cursor));
            }
            continue;
        }

        let local_byte = chunk
            .char_indices()
            .nth(remaining)
            .map(|(byte, _)| byte)
            .expect("内部不变量: remaining 小于 chunk 字符数时必定存在后续字符边界");
        return Ok(ByteOffset::new(cursor + local_byte));
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
