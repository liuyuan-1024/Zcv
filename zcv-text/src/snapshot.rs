//! Snapshot public API：提供绑定 BufferVersion 的不可变只读文本视图和坐标查询能力。
//!
//! 本文件保证后台读取可脱离可变 Buffer；它不提交编辑、不维护历史，也不暴露 Ropey 内部类型。

use std::borrow::Cow;

use crate::{
    Affinity, Anchor, BufferConfig, BufferGeneration, BufferVersion, ByteOffset, CharOffset, Line,
    LineRange, MovementDirection, MovementUnit, Position, TextChangeBatch, TextRange, TextResult,
    Utf16Offset, Utf16Position, WordBoundaryPolicy,
    errors::AnchorError,
    position_map::PositionMap,
    slicing::{LineContent, LineSlice, TextSlice},
    slicing::{
        line_content_for_text, text_range_for_byte_range, text_range_for_line,
        text_range_for_line_range,
    },
    storage::{RopeySnapshot, TextRead, text_coordinate_gateway},
    tracking::{CoordinateIndex, EditLog},
};

/// 不可变文本快照。
#[derive(Debug, Clone)]
pub struct Snapshot {
    storage: RopeySnapshot,
    version: BufferVersion,
    generation: BufferGeneration,
    config: BufferConfig,
    /// Buffer 在该版本时可见的版本化编辑日志，供 edits_since 增量同步。
    edit_log: EditLog,
    /// 不随编辑日志预算衰减的版本坐标索引，供 Anchor 与跨版本坐标解析。
    coordinate_index: CoordinateIndex,
}

impl Snapshot {
    pub(crate) fn new(
        storage: RopeySnapshot,
        version: BufferVersion,
        generation: BufferGeneration,
        config: BufferConfig,
        edit_log: EditLog,
        coordinate_index: CoordinateIndex,
    ) -> Self {
        Self {
            storage,
            version,
            generation,
            config,
            edit_log,
            coordinate_index,
        }
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    /// 当前内容代际；reset / 基线替换后与旧锚点不同。
    pub fn generation(&self) -> BufferGeneration {
        self.generation
    }

    /// 用不衰减坐标索引返回 `since` 到当前版本的坐标映射。
    ///
    /// `generation` 必须与快照一致，否则锚点已被基线替换淘汰；
    /// `since` 晚于当前版本或不在索引覆盖范围内都会显式失败。
    pub fn position_map_since(
        &self,
        generation: BufferGeneration,
        since: BufferVersion,
    ) -> TextResult<PositionMap> {
        if generation != self.generation {
            return Err(AnchorError::GenerationMismatch {
                anchor: generation,
                target: self.generation,
            }
            .into());
        }
        self.position_map_for_rebase(since)
    }

    /// 跨代际返回 `since` 到当前版本的坐标映射。
    ///
    /// 这是 reset / 基线替换后的显式重锚入口；普通解析必须走
    /// [`Snapshot::position_map_since`]，由它拒绝把旧代际当作普通版本推进。
    pub fn position_map_for_rebase(&self, since: BufferVersion) -> TextResult<PositionMap> {
        if since > self.version {
            return Err(AnchorError::TargetBeforeSource {
                anchor: since,
                target: self.version,
            }
            .into());
        }
        let patch = self
            .coordinate_index
            .patch_since(since, self.version)
            .ok_or(AnchorError::VersionNotIndexed {
                requested: since,
                current: self.version,
            })?;
        Ok(PositionMap::from_text_patch(&patch))
    }

    /// 返回自 `since` 版本到本快照版本的净编辑批次。
    ///
    /// 这是 Zed BufferSnapshot::edits_since_in_range 的本地等价：
    /// Buffer 是版本化编辑的唯一事实，组合文档不再依赖订阅的瞬时批次。
    pub fn edits_since(&self, since: BufferVersion) -> TextResult<TextChangeBatch> {
        self.edit_log.batch_since(since, self.version)
    }

    /// 自 `since` 版本到本快照版本的坐标编辑批次，不依赖会被预算裁剪的带文本编辑日志。
    ///
    /// 同代际内始终可用（坐标索引不衰减）；
    /// 跨越 reset / 基线替换返回 None，调用方必须按整体重置处理，不能把两代之间的变化当作普通增量。
    pub fn coordinate_edits_since(&self, since: BufferVersion) -> Option<TextChangeBatch> {
        if since > self.version || since < self.generation.version() {
            return None;
        }
        let patch = self.coordinate_index.patch_since(since, self.version)?;
        Some(TextChangeBatch::from_patch(
            since,
            self.version,
            patch,
            false,
        ))
    }

    /// 返回自 `since` 版本以来与 `range` 相交的编辑批次。
    pub fn edits_since_in_range(
        &self,
        since: BufferVersion,
        range: TextRange,
    ) -> TextResult<TextChangeBatch> {
        self.edit_log
            .batch_since_in_range(since, self.version, range)
    }

    /// 在 `offset` 处创建吸附到插入文本之前的锚点。
    pub fn anchor_before(&self, offset: ByteOffset) -> Anchor {
        Anchor::new(self.generation, self.version, offset).with_affinity(Affinity::Before)
    }

    /// 在 `offset` 处创建吸附到插入文本之后的锚点。
    pub fn anchor_after(&self, offset: ByteOffset) -> Anchor {
        Anchor::new(self.generation, self.version, offset).with_affinity(Affinity::After)
    }

    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    /// 按纯文本粒度查找相邻边界。
    ///
    /// 快照与可编辑 Buffer 共享同一套移动语义，使显示层可以在不可变文本视图上完成显示坐标到逻辑坐标的完整移动，不必重新取得可变 Buffer。
    pub fn movement_boundary(
        &self,
        policy: WordBoundaryPolicy,
        offset: CharOffset,
        direction: MovementDirection,
        unit: MovementUnit,
    ) -> TextResult<CharOffset> {
        crate::buffer::movement_boundary_in_text(&self.storage, policy, offset, direction, unit)
    }

    pub fn surrounding_word(
        &self,
        policy: WordBoundaryPolicy,
        offset: CharOffset,
    ) -> TextResult<(CharOffset, CharOffset)> {
        crate::buffer::surrounding_word_in_text(&self.storage, policy, offset)
    }

    pub fn is_inside_word(
        &self,
        policy: WordBoundaryPolicy,
        offset: CharOffset,
    ) -> TextResult<bool> {
        crate::buffer::is_inside_word_in_text(&self.storage, policy, offset)
    }

    // 坐标查询门面（len / byte / char / UTF-16 / grapheme 系列）与 Buffer 共用一份实现。
    text_coordinate_gateway!();

    /// `(行号, 行内 UTF-8 字节列)` 派生坐标。
    ///
    /// 与 `byte_to_position` 的区别：返回的列以 **UTF-8 字节**为单位，而不是逻辑 char column。
    /// tree-sitter `Point` 需要 byte column；该方法服务于语法高亮 producer 的 ChangeSet → InputEdit 翻译路径。
    /// 端点必须落在合法字符边界。
    pub fn byte_to_point(&self, offset: ByteOffset) -> TextResult<(Line, usize)> {
        let line = self.storage.byte_to_line(offset)?;
        let line_start = self.storage.line_start(line)?;
        Ok((line, offset.get() - line_start.get()))
    }

    /// 返回包含 `offset` 的 rope chunk 与该 chunk 在全文里的起点。
    ///
    /// 用于 tree-sitter `Parser::parse_with_options` 的 `TextProvider`回调——按 byte offset 取一段 zero-copy 文本，避免物化全文。
    /// chunk 边界落在 UTF-8 char boundary（不保证 grapheme boundary），parser 内部已能处理跨 chunk 拼接。端点必须落在合法字符边界。
    pub fn chunk_at_byte(&self, offset: ByteOffset) -> TextResult<(&str, ByteOffset)> {
        self.storage.chunk_at_byte(offset)
    }

    /// 按 byte range 读取快照文本。
    pub fn slice_text(&self, range: TextRange) -> TextResult<TextSlice<'_>> {
        Ok(TextSlice::new(range, self.storage.slice_text(range)?))
    }

    /// 按 UTF-8 byte range 读取快照文本，主要用于文件 / 外部协议适配边界。
    pub fn slice_byte_range(
        &self,
        start: ByteOffset,
        end: ByteOffset,
    ) -> TextResult<TextSlice<'_>> {
        let range = text_range_for_byte_range(&self.storage, start, end)?;
        self.slice_text(range)
    }

    /// 读取快照中的单个逻辑行；如果该行有换行符，返回文本会保留换行符。
    pub fn slice_line(&self, line: Line) -> TextResult<LineSlice<'_>> {
        let range = text_range_for_line(&self.storage, line)?;
        Ok(LineSlice::new(line, self.slice_text(range)?))
    }

    /// 按半开逻辑行区间读取快照文本。
    pub fn slice_line_range(&self, line_range: LineRange) -> TextResult<TextSlice<'_>> {
        let range = text_range_for_line_range(&self.storage, line_range)?;
        self.slice_text(range)
    }

    /// 读取快照中的单行文本内容（剥掉行尾换行符，可按 `max_line_chars` 截断）。
    ///
    /// 供软换行片段切分等读取行内容的场景使用；`None` 表示不截断。
    pub fn line_content(
        &self,
        line: Line,
        max_line_chars: Option<usize>,
    ) -> TextResult<LineContent<'_>> {
        line_content_for_text(&self.storage, line, max_line_chars)
    }
}

impl TextRead for Snapshot {
    fn slice_text(&self, range: TextRange) -> TextResult<Cow<'_, str>> {
        self.storage.slice_text(range)
    }

    fn chunks(&self, range: TextRange) -> TextResult<impl Iterator<Item = &str> + '_> {
        self.storage.chunks(range)
    }

    fn len_bytes(&self) -> ByteOffset {
        self.storage.len_bytes()
    }

    fn len_chars(&self) -> CharOffset {
        self.storage.len_chars()
    }

    fn line_count(&self) -> usize {
        self.storage.line_count()
    }

    fn line_start(&self, line: Line) -> TextResult<ByteOffset> {
        self.storage.line_start(line)
    }

    fn byte_to_position(&self, offset: ByteOffset) -> TextResult<Position> {
        self.storage.byte_to_position(offset)
    }

    fn position_to_byte(&self, position: Position) -> TextResult<ByteOffset> {
        self.storage.position_to_byte(position)
    }

    fn char_to_position(&self, offset: CharOffset) -> TextResult<Position> {
        self.storage.char_to_position(offset)
    }

    fn position_to_char(&self, position: Position) -> TextResult<CharOffset> {
        self.storage.position_to_char(position)
    }

    fn char_at(&self, offset: CharOffset) -> Option<char> {
        self.storage.char_at(offset)
    }

    fn char_at_byte(&self, offset: ByteOffset) -> Option<char> {
        self.storage.char_at_byte(offset)
    }

    fn char_to_byte(&self, offset: CharOffset) -> TextResult<ByteOffset> {
        self.storage.char_to_byte(offset)
    }

    fn byte_to_char(&self, offset: ByteOffset) -> TextResult<CharOffset> {
        self.storage.byte_to_char(offset)
    }

    fn byte_to_utf16_position(&self, offset: ByteOffset) -> TextResult<Utf16Position> {
        self.storage.byte_to_utf16_position(offset)
    }

    fn utf16_position_to_byte(&self, position: Utf16Position) -> TextResult<ByteOffset> {
        self.storage.utf16_position_to_byte(position)
    }

    fn byte_to_utf16_cu(&self, offset: ByteOffset) -> TextResult<Utf16Offset> {
        self.storage.byte_to_utf16_cu(offset)
    }

    fn utf16_cu_to_byte(&self, offset: Utf16Offset) -> TextResult<ByteOffset> {
        self.storage.utf16_cu_to_byte(offset)
    }

    fn is_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<bool> {
        self.storage.is_grapheme_boundary(offset)
    }

    fn previous_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset> {
        self.storage.previous_grapheme_boundary(offset)
    }

    fn next_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset> {
        self.storage.next_grapheme_boundary(offset)
    }

    fn line_ending_style(&self) -> crate::LineEndingStyle {
        self.storage.line_ending_style()
    }
}

#[cfg(test)]
#[path = "test/snapshot_tests.rs"]
mod tests;
