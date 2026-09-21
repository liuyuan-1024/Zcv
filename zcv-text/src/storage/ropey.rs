//! RopeyStorage 生产后端：把 `ropey` 的高性能文本结构封装成文本内核内部 TextRead 能力。
//!
//! **坐标系唯一真理**：本文件实现的 trait 以 `ByteOffset` 为深核位置类型；
//! 内部桥接 ropey 的 char-based API，对外只暴露 byte 接口（保留 char 作为边界投影）。

use std::borrow::Cow;

use ropey::Rope;
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

use super::TextRead;
use crate::{
    errors::{CoordinateError, EditError, StorageError, TextResult},
    transaction::EditList,
    types::{
        ByteOffset, CharOffset, Line, LineEndingStyle, LogicalColumn, Position, TextRange,
        Utf16Offset, Utf16Position,
    },
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

/// 共享字节区间校验：长度不越界 + 端点都落在 UTF-8 字符边界。
///
/// `RopeyStorage` 与 `RopeySnapshot` 的只读与预检路径共用，行为修复只改这一处。
fn validate_byte_range_in_rope(rope: &Rope, range: TextRange) -> TextResult<()> {
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
}

impl TextRead for RopeyStorage {
    fn slice_text(&self, range: TextRange) -> TextResult<Cow<'_, str>> {
        validate_byte_range_in_rope(&self.rope, range)?;
        Ok(rope_byte_range_cow(&self.rope, range))
    }

    fn chunks(&self, range: TextRange) -> TextResult<impl Iterator<Item = &str> + '_> {
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

    fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    fn line_start(&self, line: Line) -> TextResult<ByteOffset> {
        if line.get() >= self.rope.len_lines() {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }

        Ok(ByteOffset::new(self.rope.line_to_byte(line.get())))
    }

    fn byte_to_position(&self, offset: ByteOffset) -> TextResult<Position> {
        byte_to_position_in_rope(&self.rope, offset)
    }

    fn byte_to_line(&self, offset: ByteOffset) -> TextResult<Line> {
        byte_to_line_in_rope(&self.rope, offset)
    }

    fn position_to_byte(&self, position: Position) -> TextResult<ByteOffset> {
        position_to_byte_in_rope(&self.rope, position)
    }

    fn char_to_position(&self, offset: CharOffset) -> TextResult<Position> {
        char_to_position_in_rope(&self.rope, offset)
    }

    fn position_to_char(&self, position: Position) -> TextResult<CharOffset> {
        position_to_char_in_rope(&self.rope, position)
    }

    fn char_at(&self, offset: CharOffset) -> Option<char> {
        char_at(&self.rope, offset.get())
    }

    fn char_at_byte(&self, offset: ByteOffset) -> Option<char> {
        char_at_byte(&self.rope, offset.get())
    }

    fn byte_to_char(&self, offset: ByteOffset) -> TextResult<CharOffset> {
        byte_to_char_in_rope(&self.rope, offset)
    }

    fn char_to_byte(&self, offset: CharOffset) -> TextResult<ByteOffset> {
        char_to_byte_in_rope(&self.rope, offset)
    }

    fn byte_to_utf16_position(&self, offset: ByteOffset) -> TextResult<Utf16Position> {
        let char_offset = byte_to_char_in_rope(&self.rope, offset)?;
        char_to_utf16_position_in_rope(&self.rope, char_offset)
    }

    fn utf16_position_to_byte(&self, position: Utf16Position) -> TextResult<ByteOffset> {
        let char_offset = utf16_position_to_char_in_rope(&self.rope, position)?;
        char_to_byte_in_rope(&self.rope, char_offset)
    }

    fn byte_to_utf16_cu(&self, offset: ByteOffset) -> TextResult<Utf16Offset> {
        byte_to_utf16_cu_in_rope(&self.rope, offset)
    }

    fn utf16_cu_to_byte(&self, offset: Utf16Offset) -> TextResult<ByteOffset> {
        utf16_cu_to_byte_in_rope(&self.rope, offset)
    }

    fn is_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<bool> {
        is_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn previous_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset> {
        previous_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn next_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset> {
        next_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn line_ending_style(&self) -> LineEndingStyle {
        detect_line_ending_style_in_rope(&self.rope)
    }
}

impl RopeyStorage {
    /// 创建基于 `Rope::clone()` 的低成本不可变快照。
    pub(crate) fn snapshot(&self) -> RopeySnapshot {
        RopeySnapshot {
            rope: self.rope.clone(),
        }
    }

    /// 预检一次替换。`range` 端点必须落在 UTF-8 字符边界。
    ///
    /// 所有可能失败的后端校验、坐标换算和容量预约都必须发生在这里，
    /// 事务管线进入实际文本变异后只能调用不可失败的 `replace_prepared`。
    pub(crate) fn prepare_replace(
        &self,
        range: TextRange,
        _replacement: &str,
    ) -> TextResult<RopeyPreparedReplace> {
        validate_byte_range_in_rope(&self.rope, range)?;

        let start_byte = range.start().get();
        let end_byte = range.end().get();
        Ok(RopeyPreparedReplace {
            start_char: self.rope.byte_to_char(start_byte),
            end_char: self.rope.byte_to_char(end_byte),
        })
    }

    /// 执行已经 `prepare_replace` 预检过的替换。
    ///
    /// 调用方必须按旧文本坐标的倒序应用 prepared edits，使每个 prepared range
    /// 在当前文本中仍指向同一段旧文本。该 primitive 不返回 `Result`，从而保护事务
    /// 提交阶段不会在半提交后才发现可恢复错误。
    pub(crate) fn replace_prepared(&mut self, prepared: RopeyPreparedReplace, replacement: &str) {
        if prepared.start_char != prepared.end_char {
            self.rope.remove(prepared.start_char..prepared.end_char);
        }

        if !replacement.is_empty() {
            self.rope.insert(prepared.start_char, replacement);
        }
    }

    /// 把编辑列表应用到当前存储，坐标以当前文本为基准。
    ///
    /// 先完成全部可失败的边界预检与 byte→char 换算，再按旧文本坐标倒序执行替换，
    /// 与事务提交路径共用同一条 prepared-replace 纪律；提交阶段因此不会出现半提交。
    pub(crate) fn apply_edit_list(&mut self, edits: &EditList) -> TextResult<()> {
        let mut prepared = Vec::new();
        prepared
            .try_reserve(edits.len())
            .map_err(|_| StorageError::OutOfMemory)?;

        for edit in edits.as_slice() {
            prepared.push(self.prepare_replace(edit.range(), edit.replacement())?);
        }

        for (edit, prepared) in edits
            .as_slice()
            .iter()
            .rev()
            .zip(prepared.into_iter().rev())
        {
            self.replace_prepared(prepared, edit.replacement());
        }

        Ok(())
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
    /// 返回包含给定 byte offset 的 chunk 与该 chunk 在全文里的起点。
    ///
    /// `offset` 越界视为指向末端：仍返回最后一段 chunk 与其起点，对应 `Rope::chunk_at_byte` 的语义。
    /// chunk 边界落在 char boundary，但**不**保证 grapheme boundary——这是 tree-sitter `parse_with_options` 的契约：
    /// 调用方可以按任意 UTF-8 字节边界续读，parser 自己处理跨 chunk 拼接。
    pub(crate) fn chunk_at_byte(&self, offset: ByteOffset) -> TextResult<(&str, ByteOffset)> {
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

    /// 以当前快照文本创建一个可变异存储副本。
    ///
    /// 供后台派生与按旧版本重建文本使用；`Rope::clone()` 是低成本共享，不复制全文。
    pub(crate) fn to_storage(&self) -> RopeyStorage {
        RopeyStorage {
            rope: self.rope.clone(),
        }
    }
}

impl TextRead for RopeySnapshot {
    fn slice_text(&self, range: TextRange) -> TextResult<Cow<'_, str>> {
        validate_byte_range_in_rope(&self.rope, range)?;
        Ok(rope_byte_range_cow(&self.rope, range))
    }

    fn chunks(&self, range: TextRange) -> TextResult<impl Iterator<Item = &str> + '_> {
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

    fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    fn line_start(&self, line: Line) -> TextResult<ByteOffset> {
        if line.get() >= self.rope.len_lines() {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }

        Ok(ByteOffset::new(self.rope.line_to_byte(line.get())))
    }

    fn byte_to_position(&self, offset: ByteOffset) -> TextResult<Position> {
        byte_to_position_in_rope(&self.rope, offset)
    }

    fn byte_to_line(&self, offset: ByteOffset) -> TextResult<Line> {
        byte_to_line_in_rope(&self.rope, offset)
    }

    fn position_to_byte(&self, position: Position) -> TextResult<ByteOffset> {
        position_to_byte_in_rope(&self.rope, position)
    }

    fn char_to_position(&self, offset: CharOffset) -> TextResult<Position> {
        char_to_position_in_rope(&self.rope, offset)
    }

    fn position_to_char(&self, position: Position) -> TextResult<CharOffset> {
        position_to_char_in_rope(&self.rope, position)
    }

    fn char_at(&self, offset: CharOffset) -> Option<char> {
        char_at(&self.rope, offset.get())
    }

    fn char_at_byte(&self, offset: ByteOffset) -> Option<char> {
        char_at_byte(&self.rope, offset.get())
    }

    fn byte_to_char(&self, offset: ByteOffset) -> TextResult<CharOffset> {
        byte_to_char_in_rope(&self.rope, offset)
    }

    fn char_to_byte(&self, offset: CharOffset) -> TextResult<ByteOffset> {
        char_to_byte_in_rope(&self.rope, offset)
    }

    fn byte_to_utf16_position(&self, offset: ByteOffset) -> TextResult<Utf16Position> {
        let char_offset = byte_to_char_in_rope(&self.rope, offset)?;
        char_to_utf16_position_in_rope(&self.rope, char_offset)
    }

    fn utf16_position_to_byte(&self, position: Utf16Position) -> TextResult<ByteOffset> {
        let char_offset = utf16_position_to_char_in_rope(&self.rope, position)?;
        char_to_byte_in_rope(&self.rope, char_offset)
    }

    fn byte_to_utf16_cu(&self, offset: ByteOffset) -> TextResult<Utf16Offset> {
        byte_to_utf16_cu_in_rope(&self.rope, offset)
    }

    fn utf16_cu_to_byte(&self, offset: Utf16Offset) -> TextResult<ByteOffset> {
        utf16_cu_to_byte_in_rope(&self.rope, offset)
    }

    fn is_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<bool> {
        is_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn previous_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset> {
        previous_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn next_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset> {
        next_grapheme_boundary_in_rope(&self.rope, offset)
    }

    fn line_ending_style(&self) -> LineEndingStyle {
        detect_line_ending_style_in_rope(&self.rope)
    }
}

// ============================================================
// 核心 byte-native helper：byte_to_position / position_to_byte
// ============================================================

fn byte_to_position_in_rope(rope: &Rope, offset: ByteOffset) -> TextResult<Position> {
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

fn position_to_byte_in_rope(rope: &Rope, position: Position) -> TextResult<ByteOffset> {
    let char_offset = position_to_char_in_rope(rope, position)?;
    Ok(ByteOffset::new(rope.char_to_byte(char_offset.get())))
}

// ============================================================
// 边界投影 helper：char_to_position / position_to_char
// ============================================================

fn char_to_position_in_rope(rope: &Rope, offset: CharOffset) -> TextResult<Position> {
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

fn position_to_char_in_rope(rope: &Rope, position: Position) -> TextResult<CharOffset> {
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

fn char_to_utf16_position_in_rope(rope: &Rope, offset: CharOffset) -> TextResult<Utf16Position> {
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

fn utf16_position_to_char_in_rope(rope: &Rope, position: Utf16Position) -> TextResult<CharOffset> {
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
fn byte_to_utf16_cu_in_rope(rope: &Rope, offset: ByteOffset) -> TextResult<Utf16Offset> {
    let char_offset = byte_to_char_in_rope(rope, offset)?;
    Ok(Utf16Offset::new(rope.char_to_utf16_cu(char_offset.get())))
}

/// 全文累计 UTF-16 code unit 数 → Byte 偏移。
///
/// `offset` 落在 surrogate pair 中间属于非法边界（NSTextInputClient 不应这样
/// 调，但仍要防御）。`Rope::utf16_cu_to_char` 对 surrogate 中间会"舍入到字符
/// 起点"，因此我们事后再用 `char_to_utf16_cu` 回算一遍校验：若不相等，说明
/// 原 offset 落在 surrogate 内部，按非法边界报错。越界返回 OutOfBounds。
fn utf16_cu_to_byte_in_rope(rope: &Rope, offset: Utf16Offset) -> TextResult<ByteOffset> {
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

fn char_to_byte_in_rope(rope: &Rope, offset: CharOffset) -> TextResult<ByteOffset> {
    let char_offset = offset.get();
    if char_offset > rope.len_chars() {
        return Err(CoordinateError::CharOutOfBounds(offset).into());
    }

    Ok(ByteOffset::new(rope.char_to_byte(char_offset)))
}

fn byte_to_char_in_rope(rope: &Rope, offset: ByteOffset) -> TextResult<CharOffset> {
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
fn byte_to_line_in_rope(rope: &Rope, offset: ByteOffset) -> TextResult<Line> {
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

fn is_grapheme_boundary_in_rope(rope: &Rope, offset: ByteOffset) -> TextResult<bool> {
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

fn previous_grapheme_boundary_in_rope(rope: &Rope, offset: ByteOffset) -> TextResult<ByteOffset> {
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

fn next_grapheme_boundary_in_rope(rope: &Rope, offset: ByteOffset) -> TextResult<ByteOffset> {
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
