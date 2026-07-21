//! RopeyStorage 生产后端：把 `ropey` 的高性能文本结构封装成引擎内部 TextRead/TextStorage 能力。
//!
//! **坐标系唯一真理**：本文件实现的 trait 以 `ByteOffset` 为深核位置类型；
//! 内部桥接 ropey 的 char-based API，对外只暴露 byte 接口（保留 char 作为边界投影）。

use std::borrow::Cow;

use ropey::Rope;
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

use super::{TextFingerprint, TextRead, TextStorage};
use crate::{
    ByteOffset, CharOffset, CoordinateError, EditError, EngineResult, Line, LineEndingStyle,
    LogicalColumn, Position, TextRange, Utf16Offset, Utf16Position,
};

#[inline]
fn rope_len_bytes(rope: &Rope) -> ByteOffset {
    ByteOffset::new(rope.len_bytes())
}

/// 字节区间 Cow：单块时零拷贝，多块时才物化。调用方需保证区间已被校验。
#[inline]
fn rope_byte_range_cow(rope: &Rope, range: TextRange) -> Cow<'_, str> {
    let slice = rope.byte_slice(range.start().get()..range.end().get());
    match slice.as_str() {
        Some(s) => Cow::Borrowed(s),
        None => Cow::Owned(slice.to_string()),
    }
}

/// 共享字节区间校验（用于 Snapshot 实现，避免重复实现 validate_byte_range）。
fn validate_byte_range_in_rope(rope: &Rope, range: TextRange) -> EngineResult<()> {
    if range.end().get() > rope.len_bytes() {
        return Err(EditError::RangeOutOfBounds { range }.into());
    }
    if !is_utf8_char_boundary_in_rope(rope, range.start().get()) {
        return Err(CoordinateError::InvalidByteBoundary(range.start()).into());
    }
    if !is_utf8_char_boundary_in_rope(rope, range.end().get()) {
        return Err(CoordinateError::InvalidByteBoundary(range.end()).into());
    }
    Ok(())
}

#[inline]
fn rope_len_utf16_cu(rope: &Rope) -> Utf16Offset {
    Utf16Offset::new(rope.len_utf16_cu())
}

/// 默认高性能文本后端。
///
/// 不把 `ropey::Rope` 暴露到 public API；外部仍然只看到 Buffer / Snapshot / ByteOffset。
#[derive(Debug, Clone)]
pub(crate) struct RopeyStorage {
    rope: Rope,
}

/// `RopeyStorage` 已完成预检的替换坐标。
///
/// 这里保存 ropey 原生 char range，使事务提交阶段不再做任何可失败的
/// byte 边界校验或 byte→char 坐标换算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RopeyPreparedReplace {
    start_char: usize,
    end_char: usize,
}

impl RopeyStorage {
    pub(crate) fn new(text: String) -> Self {
        Self {
            rope: Rope::from_str(&text),
        }
    }

    /// 从已构建的 `Rope` 直接接管所有权，避免再做一次全量 `from_str`。
    ///
    /// 给流式加载路径（[`crate::Buffer::from_reader`]）走——decoder 已经把
    /// 字节增量喂进 `RopeBuilder`，`finish()` 返回 `Rope` 后我们只是把它装进
    /// storage，不再二次拷贝。
    pub(crate) fn from_rope(rope: Rope) -> Self {
        Self { rope }
    }

    pub(crate) fn fingerprint(&self) -> TextFingerprint {
        fingerprint_rope(&self.rope)
    }

    pub(crate) fn has_same_text(&self, snapshot: &RopeySnapshot) -> bool {
        ropes_have_same_text(&self.rope, &snapshot.rope)
    }

    /// 校验字节区间：长度不越界 + 端点都落在 UTF-8 字符边界 + 不切 CRLF 中间。
    fn validate_byte_range(&self, range: TextRange) -> EngineResult<()> {
        let len_bytes = self.rope.len_bytes();

        if range.end().get() > len_bytes {
            return Err(EditError::RangeOutOfBounds { range }.into());
        }

        if !is_utf8_char_boundary_in_rope(&self.rope, range.start().get()) {
            return Err(CoordinateError::InvalidByteBoundary(range.start()).into());
        }

        if !is_utf8_char_boundary_in_rope(&self.rope, range.end().get()) {
            return Err(CoordinateError::InvalidByteBoundary(range.end()).into());
        }

        Ok(())
    }
}

impl TextRead for RopeyStorage {
    fn slice_text(&self, range: TextRange) -> EngineResult<Cow<'_, str>> {
        self.validate_byte_range(range)?;
        Ok(rope_byte_range_cow(&self.rope, range))
    }

    fn chunks(&self, range: TextRange) -> EngineResult<impl Iterator<Item = &str> + '_> {
        self.validate_byte_range(range)?;
        Ok(self
            .rope
            .byte_slice(range.start().get()..range.end().get())
            .chunks())
    }

    fn len_bytes(&self) -> ByteOffset {
        rope_len_bytes(&self.rope)
    }

    fn len_chars(&self) -> CharOffset {
        CharOffset::new(self.rope.len_chars())
    }

    fn len_utf16_cu(&self) -> Utf16Offset {
        rope_len_utf16_cu(&self.rope)
    }

    fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    fn line_start(&self, line: Line) -> EngineResult<ByteOffset> {
        if line.get() >= self.rope.len_lines() {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }

        Ok(ByteOffset::new(self.rope.line_to_byte(line.get())))
    }

    fn byte_to_position(&self, offset: ByteOffset) -> EngineResult<Position> {
        byte_to_position_in_rope(&self.rope, offset)
    }

    fn byte_to_line(&self, offset: ByteOffset) -> EngineResult<Line> {
        byte_to_line_in_rope(&self.rope, offset)
    }

    fn position_to_byte(&self, position: Position) -> EngineResult<ByteOffset> {
        position_to_byte_in_rope(&self.rope, position)
    }

    fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position> {
        char_to_position_in_rope(&self.rope, offset)
    }

    fn position_to_char(&self, position: Position) -> EngineResult<CharOffset> {
        position_to_char_in_rope(&self.rope, position)
    }

    fn char_at(&self, offset: CharOffset) -> Option<char> {
        char_at(&self.rope, offset.get())
    }

    fn char_at_byte(&self, offset: ByteOffset) -> Option<char> {
        char_at_byte(&self.rope, offset.get())
    }

    fn byte_to_char(&self, offset: ByteOffset) -> EngineResult<CharOffset> {
        byte_to_char_in_rope(&self.rope, offset)
    }

    fn char_to_byte(&self, offset: CharOffset) -> EngineResult<ByteOffset> {
        char_to_byte_in_rope(&self.rope, offset)
    }

    fn byte_to_utf16_position(&self, offset: ByteOffset) -> EngineResult<Utf16Position> {
        let char_offset = byte_to_char_in_rope(&self.rope, offset)?;
        char_to_utf16_position_in_rope(&self.rope, char_offset)
    }

    fn utf16_position_to_byte(&self, position: Utf16Position) -> EngineResult<ByteOffset> {
        let char_offset = utf16_position_to_char_in_rope(&self.rope, position)?;
        char_to_byte_in_rope(&self.rope, char_offset)
    }

    fn byte_to_utf16_cu(&self, offset: ByteOffset) -> EngineResult<Utf16Offset> {
        byte_to_utf16_cu_in_rope(&self.rope, offset)
    }

    fn utf16_cu_to_byte(&self, offset: Utf16Offset) -> EngineResult<ByteOffset> {
        utf16_cu_to_byte_in_rope(&self.rope, offset)
    }

    fn is_grapheme_boundary(&self, offset: ByteOffset) -> EngineResult<bool> {
        is_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn previous_grapheme_boundary(&self, offset: ByteOffset) -> EngineResult<ByteOffset> {
        previous_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn next_grapheme_boundary(&self, offset: ByteOffset) -> EngineResult<ByteOffset> {
        next_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn line_ending_style(&self) -> LineEndingStyle {
        detect_line_ending_style_in_rope(&self.rope)
    }
}

impl TextStorage for RopeyStorage {
    type Snapshot = RopeySnapshot;
    type PreparedReplace = RopeyPreparedReplace;

    fn snapshot(&self) -> Self::Snapshot {
        RopeySnapshot {
            rope: self.rope.clone(),
        }
    }

    fn prepare_replace(
        &self,
        range: TextRange,
        _replacement: &str,
    ) -> EngineResult<Self::PreparedReplace> {
        self.validate_byte_range(range)?;

        let start_byte = range.start().get();
        let end_byte = range.end().get();
        Ok(RopeyPreparedReplace {
            start_char: self.rope.byte_to_char(start_byte),
            end_char: self.rope.byte_to_char(end_byte),
        })
    }

    fn replace_prepared(&mut self, prepared: Self::PreparedReplace, replacement: &str) {
        if prepared.start_char != prepared.end_char {
            self.rope.remove(prepared.start_char..prepared.end_char);
        }

        if !replacement.is_empty() {
            self.rope.insert(prepared.start_char, replacement);
        }
    }
}

/// Ropey-backed 不可变快照。
///
/// 通过 `Rope::clone()` 共享底层数据，构造成本与文本长度无关。
#[derive(Debug, Clone)]
pub(crate) struct RopeySnapshot {
    rope: Rope,
}

impl RopeySnapshot {
    pub(crate) fn fingerprint(&self) -> TextFingerprint {
        fingerprint_rope(&self.rope)
    }

    /// 返回包含给定 byte offset 的 chunk 与该 chunk 在全文里的起点。
    ///
    /// `offset` 越界视为指向末端：仍返回最后一段 chunk 与其起点，对应 `Rope::chunk_at_byte` 的语义。
    /// chunk 边界落在 char boundary，但**不**保证 grapheme boundary——这是 tree-sitter `parse_with_options` 的契约：
    /// 调用方可以按任意 UTF-8 字节边界续读，parser 自己处理跨 chunk 拼接。
    pub(crate) fn chunk_at_byte(&self, offset: ByteOffset) -> EngineResult<(&str, ByteOffset)> {
        let byte_offset = offset.get();
        if byte_offset > self.rope.len_bytes() {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }
        if !is_utf8_char_boundary_in_rope(&self.rope, byte_offset) {
            return Err(CoordinateError::InvalidByteBoundary(offset).into());
        }
        let (chunk, chunk_start, _, _) = self.rope.chunk_at_byte(byte_offset);
        Ok((chunk, ByteOffset::new(chunk_start)))
    }
}

impl TextRead for RopeySnapshot {
    fn slice_text(&self, range: TextRange) -> EngineResult<Cow<'_, str>> {
        validate_byte_range_in_rope(&self.rope, range)?;
        Ok(rope_byte_range_cow(&self.rope, range))
    }

    fn chunks(&self, range: TextRange) -> EngineResult<impl Iterator<Item = &str> + '_> {
        validate_byte_range_in_rope(&self.rope, range)?;
        Ok(self
            .rope
            .byte_slice(range.start().get()..range.end().get())
            .chunks())
    }

    fn len_bytes(&self) -> ByteOffset {
        rope_len_bytes(&self.rope)
    }

    fn len_chars(&self) -> CharOffset {
        CharOffset::new(self.rope.len_chars())
    }

    fn len_utf16_cu(&self) -> Utf16Offset {
        rope_len_utf16_cu(&self.rope)
    }

    fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    fn line_start(&self, line: Line) -> EngineResult<ByteOffset> {
        if line.get() >= self.rope.len_lines() {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }

        Ok(ByteOffset::new(self.rope.line_to_byte(line.get())))
    }

    fn byte_to_position(&self, offset: ByteOffset) -> EngineResult<Position> {
        byte_to_position_in_rope(&self.rope, offset)
    }

    fn byte_to_line(&self, offset: ByteOffset) -> EngineResult<Line> {
        byte_to_line_in_rope(&self.rope, offset)
    }

    fn position_to_byte(&self, position: Position) -> EngineResult<ByteOffset> {
        position_to_byte_in_rope(&self.rope, position)
    }

    fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position> {
        char_to_position_in_rope(&self.rope, offset)
    }

    fn position_to_char(&self, position: Position) -> EngineResult<CharOffset> {
        position_to_char_in_rope(&self.rope, position)
    }

    fn char_at(&self, offset: CharOffset) -> Option<char> {
        char_at(&self.rope, offset.get())
    }

    fn char_at_byte(&self, offset: ByteOffset) -> Option<char> {
        char_at_byte(&self.rope, offset.get())
    }

    fn byte_to_char(&self, offset: ByteOffset) -> EngineResult<CharOffset> {
        byte_to_char_in_rope(&self.rope, offset)
    }

    fn char_to_byte(&self, offset: CharOffset) -> EngineResult<ByteOffset> {
        char_to_byte_in_rope(&self.rope, offset)
    }

    fn byte_to_utf16_position(&self, offset: ByteOffset) -> EngineResult<Utf16Position> {
        let char_offset = byte_to_char_in_rope(&self.rope, offset)?;
        char_to_utf16_position_in_rope(&self.rope, char_offset)
    }

    fn utf16_position_to_byte(&self, position: Utf16Position) -> EngineResult<ByteOffset> {
        let char_offset = utf16_position_to_char_in_rope(&self.rope, position)?;
        char_to_byte_in_rope(&self.rope, char_offset)
    }

    fn byte_to_utf16_cu(&self, offset: ByteOffset) -> EngineResult<Utf16Offset> {
        byte_to_utf16_cu_in_rope(&self.rope, offset)
    }

    fn utf16_cu_to_byte(&self, offset: Utf16Offset) -> EngineResult<ByteOffset> {
        utf16_cu_to_byte_in_rope(&self.rope, offset)
    }

    fn is_grapheme_boundary(&self, offset: ByteOffset) -> EngineResult<bool> {
        is_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn previous_grapheme_boundary(&self, offset: ByteOffset) -> EngineResult<ByteOffset> {
        previous_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn next_grapheme_boundary(&self, offset: ByteOffset) -> EngineResult<ByteOffset> {
        next_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn line_ending_style(&self) -> LineEndingStyle {
        detect_line_ending_style_in_rope(&self.rope)
    }
}

// ============================================================
// 核心 byte-native helper：byte_to_position / position_to_byte
// ============================================================

fn byte_to_position_in_rope(rope: &Rope, offset: ByteOffset) -> EngineResult<Position> {
    let byte_offset = offset.get();
    if byte_offset > rope.len_bytes() {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }
    if !is_utf8_char_boundary_in_rope(rope, byte_offset) {
        return Err(CoordinateError::InvalidByteBoundary(offset).into());
    }
    let char_offset = rope.byte_to_char(byte_offset);

    if is_crlf_middle(rope, char_offset) {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    let line_idx = rope.char_to_line(char_offset);
    let line_char_start = rope.line_to_char(line_idx);
    let column = char_offset - line_char_start;

    Ok(Position::new(
        Line::new(line_idx),
        LogicalColumn::new(column),
    ))
}

fn position_to_byte_in_rope(rope: &Rope, position: Position) -> EngineResult<ByteOffset> {
    let char_offset = position_to_char_in_rope(rope, position)?;
    Ok(ByteOffset::new(rope.char_to_byte(char_offset.get())))
}

// ============================================================
// 边界投影 helper：char_to_position / position_to_char
// ============================================================

fn char_to_position_in_rope(rope: &Rope, offset: CharOffset) -> EngineResult<Position> {
    let offset_value = offset.get();
    if offset_value > rope.len_chars() {
        return Err(CoordinateError::CharOutOfBounds(offset).into());
    }

    if is_crlf_middle(rope, offset_value) {
        return Err(CoordinateError::CharOutOfBounds(offset).into());
    }

    let line_idx = rope.char_to_line(offset_value);
    let line_start = rope.line_to_char(line_idx);
    let column = offset_value - line_start;

    Ok(Position::new(
        Line::new(line_idx),
        LogicalColumn::new(column),
    ))
}

fn position_to_char_in_rope(rope: &Rope, position: Position) -> EngineResult<CharOffset> {
    let line = position.line();
    let column = position.column().get();

    if line.get() >= rope.len_lines() {
        return Err(CoordinateError::LineOutOfBounds(line).into());
    }

    let line_start = rope.line_to_char(line.get());
    let next_line_start = if line.get() + 1 < rope.len_lines() {
        rope.line_to_char(line.get() + 1)
    } else {
        rope.len_chars()
    };

    let line_content_end = line_content_end(rope, line_start, next_line_start);
    let line_len = line_content_end - line_start;

    if column <= line_len {
        return Ok(CharOffset::new(line_start + column));
    }

    Err(CoordinateError::CharOutOfBounds(CharOffset::new(line_content_end)).into())
}

// ============================================================
// UTF-8 / UTF-16 / Char 投影
// ============================================================

fn char_to_utf16_position_in_rope(rope: &Rope, offset: CharOffset) -> EngineResult<Utf16Position> {
    let offset_value = offset.get();

    if offset_value > rope.len_chars() {
        return Err(CoordinateError::CharOutOfBounds(offset).into());
    }

    if is_crlf_middle(rope, offset_value) {
        return Err(CoordinateError::CharOutOfBounds(offset).into());
    }

    let line_idx = rope.char_to_line(offset_value);
    let line_start = rope.line_to_char(line_idx);
    let utf16_units = rope
        .slice(line_start..offset_value)
        .chars()
        .map(char::len_utf16)
        .sum();

    Ok(Utf16Position::new(
        Line::new(line_idx),
        Utf16Offset::new(utf16_units),
    ))
}

fn utf16_position_to_char_in_rope(
    rope: &Rope,
    position: Utf16Position,
) -> EngineResult<CharOffset> {
    let line = position.line();

    if line.get() >= rope.len_lines() {
        return Err(CoordinateError::LineOutOfBounds(line).into());
    }

    let line_start = rope.line_to_char(line.get());
    let next_line_start = if line.get() + 1 < rope.len_lines() {
        rope.line_to_char(line.get() + 1)
    } else {
        rope.len_chars()
    };
    let line_content_end = line_content_end(rope, line_start, next_line_start);
    let target = position.character().get();

    let mut utf16_units = 0usize;
    let mut char_count = 0usize;

    if target == 0 {
        return Ok(CharOffset::new(line_start));
    }

    for ch in rope.slice(line_start..line_content_end).chars() {
        let next_utf16_units = utf16_units + ch.len_utf16();
        let next_char_count = char_count + 1;

        if target == next_utf16_units {
            return Ok(CharOffset::new(line_start + next_char_count));
        }

        if target < next_utf16_units {
            return Err(CoordinateError::InvalidUtf16Boundary(position).into());
        }

        utf16_units = next_utf16_units;
        char_count = next_char_count;
    }

    if target == utf16_units {
        return Ok(CharOffset::new(line_start + char_count));
    }

    Err(CoordinateError::Utf16PositionOutOfBounds(position).into())
}

/// Byte 偏移 → 全文累计 UTF-16 code unit 数。
///
/// O(log n)：先 byte→char（rope 原生），再走 rope 的 `char_to_utf16_cu`
/// 用内部累计索引一步到位，**不拷贝任何文本**——这是 IME 大文件能跑得动
/// 的关键路径。
fn byte_to_utf16_cu_in_rope(rope: &Rope, offset: ByteOffset) -> EngineResult<Utf16Offset> {
    let char_offset = byte_to_char_in_rope(rope, offset)?;
    Ok(Utf16Offset::new(rope.char_to_utf16_cu(char_offset.get())))
}

/// 全文累计 UTF-16 code unit 数 → Byte 偏移。
///
/// `offset` 落在 surrogate pair 中间属于非法边界（NSTextInputClient 不应这样
/// 调，但仍要防御）。`Rope::utf16_cu_to_char` 对 surrogate 中间会"舍入到字符
/// 起点"，因此我们事后再用 `char_to_utf16_cu` 回算一遍校验：若不相等，说明
/// 原 offset 落在 surrogate 内部，按非法边界报错。越界返回 OutOfBounds。
fn utf16_cu_to_byte_in_rope(rope: &Rope, offset: Utf16Offset) -> EngineResult<ByteOffset> {
    let target = offset.get();
    if target > rope.len_utf16_cu() {
        return Err(
            CoordinateError::Utf16PositionOutOfBounds(Utf16Position::new(Line::ZERO, offset))
                .into(),
        );
    }
    let char_idx = rope.utf16_cu_to_char(target);
    let roundtrip = rope.char_to_utf16_cu(char_idx);
    if roundtrip != target {
        return Err(
            CoordinateError::InvalidUtf16Boundary(Utf16Position::new(Line::ZERO, offset)).into(),
        );
    }
    Ok(ByteOffset::new(rope.char_to_byte(char_idx)))
}

fn char_to_byte_in_rope(rope: &Rope, offset: CharOffset) -> EngineResult<ByteOffset> {
    let char_offset = offset.get();
    if char_offset > rope.len_chars() {
        return Err(CoordinateError::CharOutOfBounds(offset).into());
    }

    Ok(ByteOffset::new(rope.char_to_byte(char_offset)))
}

fn byte_to_char_in_rope(rope: &Rope, offset: ByteOffset) -> EngineResult<CharOffset> {
    let byte_offset = offset.get();

    if byte_offset > rope.len_bytes() {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    if !is_utf8_char_boundary_in_rope(rope, byte_offset) {
        return Err(CoordinateError::InvalidByteBoundary(offset).into());
    }

    Ok(CharOffset::new(rope.byte_to_char(byte_offset)))
}

/// `byte_to_position` 的省列变体：单次 `rope.byte_to_line` 调用，省掉
/// `byte_to_char → is_crlf_middle → char_to_line → line_to_char` 这条链路里
/// 后三段的额外 O(log N)。CRLF 中点检测在此放宽：调用方场景（fold 几何）只关心
/// 行号且字节区间已在 fold 创建处校验过；返回的行号以 `\n` 为分界，与外部协议一致。
fn byte_to_line_in_rope(rope: &Rope, offset: ByteOffset) -> EngineResult<Line> {
    let byte_offset = offset.get();

    if byte_offset > rope.len_bytes() {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    if !is_utf8_char_boundary_in_rope(rope, byte_offset) {
        return Err(CoordinateError::InvalidByteBoundary(offset).into());
    }

    Ok(Line::new(rope.byte_to_line(byte_offset)))
}

fn is_utf8_char_boundary_in_rope(rope: &Rope, byte_offset: usize) -> bool {
    if byte_offset == 0 {
        return true;
    }
    if byte_offset > rope.len_bytes() {
        return false;
    }
    if byte_offset == rope.len_bytes() {
        return true;
    }

    let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte_offset);
    chunk.is_char_boundary(byte_offset - chunk_start)
}

// ============================================================
// Grapheme cluster 边界（byte-native）
// ============================================================

fn is_grapheme_boundary_in_rope(rope: &Rope, offset: ByteOffset) -> EngineResult<bool> {
    let byte_offset = offset.get();
    if byte_offset > rope.len_bytes() {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }
    if !is_utf8_char_boundary_in_rope(rope, byte_offset) {
        return Err(CoordinateError::InvalidByteBoundary(offset).into());
    }

    let mut cursor = GraphemeCursor::new(byte_offset, rope.len_bytes(), true);
    let (mut chunk, mut chunk_start, _, _) = if byte_offset < rope.len_bytes() {
        rope.chunk_at_byte(byte_offset)
    } else if byte_offset == 0 {
        ("", 0, 0, 0)
    } else {
        rope.chunk_at_byte(byte_offset.saturating_sub(1))
    };

    loop {
        match cursor.is_boundary(chunk, chunk_start) {
            Ok(result) => return Ok(result),
            Err(GraphemeIncomplete::PreContext(context_offset)) => {
                let context_index = context_offset.saturating_sub(1);
                let (context_chunk, context_start, _, _) = rope.chunk_at_byte(context_index);
                cursor.provide_context(context_chunk, context_start);
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                if chunk_start == 0 {
                    return Ok(true);
                }

                let (prev_chunk, prev_start, _, _) = rope.chunk_at_byte(chunk_start - 1);
                chunk = prev_chunk;
                chunk_start = prev_start;
            }
            Err(GraphemeIncomplete::NextChunk) => {
                let next_start = chunk_start + chunk.len();
                if next_start >= rope.len_bytes() {
                    return Ok(true);
                }

                let (next_chunk, next_chunk_start, _, _) = rope.chunk_at_byte(next_start);
                chunk = next_chunk;
                chunk_start = next_chunk_start;
            }
            Err(GraphemeIncomplete::InvalidOffset) => {
                return Err(CoordinateError::InvalidGraphemeBoundary(offset).into());
            }
        }
    }
}

fn previous_grapheme_boundary_in_rope(rope: &Rope, offset: ByteOffset) -> EngineResult<ByteOffset> {
    let byte_offset = offset.get();
    if byte_offset > rope.len_bytes() {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }
    if !is_utf8_char_boundary_in_rope(rope, byte_offset) {
        return Err(CoordinateError::InvalidByteBoundary(offset).into());
    }

    let mut cursor = GraphemeCursor::new(byte_offset, rope.len_bytes(), true);
    let (mut chunk, mut chunk_start, _, _) = if byte_offset < rope.len_bytes() {
        rope.chunk_at_byte(byte_offset)
    } else if byte_offset == 0 {
        ("", 0, 0, 0)
    } else {
        rope.chunk_at_byte(byte_offset.saturating_sub(1))
    };

    loop {
        match cursor.prev_boundary(chunk, chunk_start) {
            Ok(Some(boundary)) => return Ok(ByteOffset::new(boundary)),
            Ok(None) => return Ok(ByteOffset::ZERO),
            Err(GraphemeIncomplete::PreContext(context_offset)) => {
                let context_index = context_offset.saturating_sub(1);
                let (context_chunk, context_start, _, _) = rope.chunk_at_byte(context_index);
                cursor.provide_context(context_chunk, context_start);
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                if chunk_start == 0 {
                    return Ok(ByteOffset::ZERO);
                }

                let (prev_chunk, prev_start, _, _) = rope.chunk_at_byte(chunk_start - 1);
                chunk = prev_chunk;
                chunk_start = prev_start;
            }
            Err(GraphemeIncomplete::NextChunk) => {
                let next_start = chunk_start + chunk.len();
                if next_start >= rope.len_bytes() {
                    return Ok(ByteOffset::new(rope.len_bytes()));
                }

                let (next_chunk, next_chunk_start, _, _) = rope.chunk_at_byte(next_start);
                chunk = next_chunk;
                chunk_start = next_chunk_start;
            }
            Err(GraphemeIncomplete::InvalidOffset) => {
                return Err(CoordinateError::InvalidGraphemeBoundary(offset).into());
            }
        }
    }
}

fn next_grapheme_boundary_in_rope(rope: &Rope, offset: ByteOffset) -> EngineResult<ByteOffset> {
    let byte_offset = offset.get();
    if byte_offset > rope.len_bytes() {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }
    if !is_utf8_char_boundary_in_rope(rope, byte_offset) {
        return Err(CoordinateError::InvalidByteBoundary(offset).into());
    }

    let mut cursor = GraphemeCursor::new(byte_offset, rope.len_bytes(), true);
    let (mut chunk, mut chunk_start, _, _) = if byte_offset < rope.len_bytes() {
        rope.chunk_at_byte(byte_offset)
    } else if byte_offset == 0 {
        ("", 0, 0, 0)
    } else {
        rope.chunk_at_byte(byte_offset.saturating_sub(1))
    };

    loop {
        match cursor.next_boundary(chunk, chunk_start) {
            Ok(Some(boundary)) => return Ok(ByteOffset::new(boundary)),
            Ok(None) => return Ok(ByteOffset::new(rope.len_bytes())),
            Err(GraphemeIncomplete::PreContext(context_offset)) => {
                let context_index = context_offset.saturating_sub(1);
                let (context_chunk, context_start, _, _) = rope.chunk_at_byte(context_index);
                cursor.provide_context(context_chunk, context_start);
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                if chunk_start == 0 {
                    return Ok(ByteOffset::ZERO);
                }

                let (prev_chunk, prev_start, _, _) = rope.chunk_at_byte(chunk_start - 1);
                chunk = prev_chunk;
                chunk_start = prev_start;
            }
            Err(GraphemeIncomplete::NextChunk) => {
                let next_start = chunk_start + chunk.len();
                if next_start >= rope.len_bytes() {
                    return Ok(ByteOffset::new(rope.len_bytes()));
                }

                let (next_chunk, next_chunk_start, _, _) = rope.chunk_at_byte(next_start);
                chunk = next_chunk;
                chunk_start = next_chunk_start;
            }
            Err(GraphemeIncomplete::InvalidOffset) => {
                return Err(CoordinateError::InvalidGraphemeBoundary(offset).into());
            }
        }
    }
}

fn detect_line_ending_style_in_rope(rope: &Rope) -> LineEndingStyle {
    let mut has_lf = false;
    let mut has_crlf = false;
    let mut has_lone_cr = false;
    let mut pending_cr = false;

    for chunk in rope.chunks() {
        for byte in chunk.as_bytes() {
            match *byte {
                b'\n' if pending_cr => {
                    has_crlf = true;
                    pending_cr = false;
                }
                b'\n' => {
                    has_lf = true;
                }
                b'\r' => {
                    if pending_cr {
                        has_lone_cr = true;
                    }
                    pending_cr = true;
                }
                _ => {
                    if pending_cr {
                        has_lone_cr = true;
                        pending_cr = false;
                    }
                }
            }
        }
    }

    if pending_cr {
        has_lone_cr = true;
    }

    match (has_lf, has_crlf, has_lone_cr) {
        (false, false, false) => LineEndingStyle::None,
        (true, false, false) => LineEndingStyle::Lf,
        (false, true, false) => LineEndingStyle::Crlf,
        _ => LineEndingStyle::Mixed,
    }
}

fn line_content_end(rope: &Rope, line_start: usize, next_line_start: usize) -> usize {
    if next_line_start <= line_start {
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

fn is_crlf_middle(rope: &Rope, char_offset: usize) -> bool {
    char_offset > 0
        && char_offset < rope.len_chars()
        && char_at(rope, char_offset - 1) == Some('\r')
        && char_at(rope, char_offset) == Some('\n')
}

fn char_at(rope: &Rope, char_offset: usize) -> Option<char> {
    if char_offset >= rope.len_chars() {
        return None;
    }

    Some(rope.char(char_offset))
}

fn char_at_byte(rope: &Rope, byte_offset: usize) -> Option<char> {
    if byte_offset >= rope.len_bytes() {
        return None;
    }
    if !is_utf8_char_boundary_in_rope(rope, byte_offset) {
        return None;
    }
    let char_offset = rope.byte_to_char(byte_offset);
    char_at(rope, char_offset)
}

fn fingerprint_rope(rope: &Rope) -> TextFingerprint {
    // 手写 FNV-1a 哈希而非引入标准 hash crate（如 fnv / rustc-hash）。
    //
    // TextFingerprint 只需要 64 位非密码学哈希用于快速文本等价性探测，不参与安全决策；
    // FNV-1a 实现仅 6 行代码，增量开销为零，且比引入一个仅暴露 pub const 的 micro crate 更符合"依赖最小化"原则。
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;

    for chunk in rope.chunks() {
        for byte in chunk.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }

    TextFingerprint::new(
        ByteOffset::new(rope.len_bytes()),
        CharOffset::new(rope.len_chars()),
        hash,
    )
}

fn ropes_have_same_text(left: &Rope, right: &Rope) -> bool {
    left.len_bytes() == right.len_bytes()
        && left.len_chars() == right.len_chars()
        && left.chars().eq(right.chars())
}
