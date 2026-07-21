//! Snapshot public API：提供绑定 BufferVersion 的不可变只读文本视图和坐标查询能力。
//!
//! 本文件保证后台读取可脱离可变 Buffer；它不提交编辑、不维护历史，也不暴露 Ropey 内部类型。

use crate::{
    BufferConfig, BufferVersion, ByteOffset, CharOffset, CoordinateError, DisplayColumn,
    DisplayColumnAffinity, EngineResult, Line, LineEndingStyle, LineRange, LineSlice,
    LogicalColumn, Position, RegexSearchOptions, RegexSearchResult, SearchHandle, SearchOptions,
    SearchResult, TextRange, TextSlice, Utf16Offset, Utf16Position, Viewport, ViewportSlice,
    VisibleLine,
    coordinates::core::{
        char_to_display_column_in_text, display_to_logical_column_in_text,
        logical_to_display_column_in_text, next_tab_stop,
    },
    slicing::{
        text_range_for_byte_range, text_range_for_line, text_range_for_line_range,
        viewport_slice_for_text, visible_line_for_text,
    },
    storage::{RopeySnapshot, TextRead},
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

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    pub fn len_chars(&self) -> CharOffset {
        self.storage.len_chars()
    }

    /// 文本 UTF-8 字节末端位置；等价于全文末尾的 `ByteOffset`。
    pub fn len_bytes(&self) -> ByteOffset {
        self.storage.len_bytes()
    }

    /// 文本 UTF-16 code unit 末端位置；等价于全文末尾的 `Utf16Offset`，
    /// 用于与 LSP / 外部协议的坐标边界对齐。
    pub fn len_utf16_cu(&self) -> Utf16Offset {
        self.storage.len_utf16_cu()
    }

    pub fn line_count(&self) -> usize {
        self.storage.line_count()
    }

    /// 指定行的起始 ByteOffset（深核接口）。
    pub fn line_start_byte(&self, line: Line) -> EngineResult<ByteOffset> {
        self.storage.line_start(line)
    }

    /// 指定行的起始 CharOffset（边界投影）。
    pub fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        let byte = self.storage.line_start(line)?;
        self.storage.byte_to_char(byte)
    }

    pub fn byte_to_position(&self, offset: ByteOffset) -> EngineResult<Position> {
        self.storage.byte_to_position(offset)
    }

    /// `byte_to_position` 的省列变体；详见 `Buffer::byte_to_line`。
    pub fn byte_to_line(&self, offset: ByteOffset) -> EngineResult<Line> {
        self.storage.byte_to_line(offset)
    }

    /// `(行号, 行内 UTF-8 字节列)` 派生坐标。
    ///
    /// 与 `byte_to_position` 的区别：返回的列以 **UTF-8 字节**为单位，而不是逻辑 char column。
    /// tree-sitter `Point` 需要 byte column；该方法服务于语法高亮 producer 的 ChangeSet → InputEdit 翻译路径。
    /// 端点必须落在合法字符边界。
    pub fn byte_to_point(&self, offset: ByteOffset) -> EngineResult<(Line, usize)> {
        let line = self.storage.byte_to_line(offset)?;
        let line_start = self.storage.line_start(line)?;
        Ok((line, offset.get() - line_start.get()))
    }

    /// 返回包含 `offset` 的 rope chunk 与该 chunk 在全文里的起点。
    ///
    /// 用于 tree-sitter `Parser::parse_with_options` 的 `TextProvider`回调——按 byte offset 取一段 zero-copy 文本，避免物化全文。
    /// chunk 边界落在 UTF-8 char boundary（不保证 grapheme boundary），parser 内部已能处理跨 chunk 拼接。端点必须落在合法字符边界。
    pub fn chunk_at_byte(&self, offset: ByteOffset) -> EngineResult<(&str, ByteOffset)> {
        self.storage.chunk_at_byte(offset)
    }

    pub fn position_to_byte(&self, position: Position) -> EngineResult<ByteOffset> {
        self.storage.position_to_byte(position)
    }

    /// 按 byte range 读取快照文本。
    pub fn slice_text(&self, range: TextRange) -> EngineResult<TextSlice<'_>> {
        Ok(TextSlice::new(range, self.storage.slice_text(range)?))
    }

    /// 按 UTF-8 byte range 读取快照文本，主要用于文件 / 外部协议适配边界。
    pub fn slice_byte_range(
        &self,
        start: ByteOffset,
        end: ByteOffset,
    ) -> EngineResult<TextSlice<'_>> {
        let range = text_range_for_byte_range(&self.storage, start, end)?;
        self.slice_text(range)
    }

    /// 读取快照中的单个逻辑行；如果该行有换行符，返回文本会保留换行符。
    pub fn slice_line(&self, line: Line) -> EngineResult<LineSlice<'_>> {
        let range = text_range_for_line(&self.storage, line)?;
        Ok(LineSlice::new(line, self.slice_text(range)?))
    }

    /// 按半开逻辑行区间读取快照文本。
    pub fn slice_line_range(&self, line_range: LineRange) -> EngineResult<TextSlice<'_>> {
        let range = text_range_for_line_range(&self.storage, line_range)?;
        self.slice_text(range)
    }

    /// 按逻辑行 viewport 读取快照中的可见行。
    pub fn slice_viewport(&self, viewport: Viewport) -> EngineResult<ViewportSlice<'_>> {
        viewport_slice_for_text(&self.storage, viewport)
    }

    pub(crate) fn visible_line(
        &self,
        line: Line,
        max_line_chars: Option<usize>,
    ) -> EngineResult<VisibleLine<'_>> {
        visible_line_for_text(&self.storage, line, max_line_chars)
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

    /// 全文 flat UTF-16 code unit 偏移：byte → utf16 cu。详见
    /// [`crate::Buffer::byte_to_utf16_cu`]。
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

    pub fn is_grapheme_boundary_byte(&self, offset: ByteOffset) -> EngineResult<bool> {
        self.storage.is_grapheme_boundary(offset)
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

    pub fn previous_grapheme_boundary_byte(&self, offset: ByteOffset) -> EngineResult<ByteOffset> {
        self.storage.previous_grapheme_boundary(offset)
    }

    pub fn next_grapheme_boundary_byte(&self, offset: ByteOffset) -> EngineResult<ByteOffset> {
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

    pub fn is_stale_for_version(&self, version: BufferVersion) -> bool {
        self.version != version
    }

    /// 在该不可变快照中启动一次 literal 搜索，结果绑定快照版本。
    ///
    /// 后台线程持有当前快照的克隆，因此调用方可以立即继续使用 Buffer 或丢弃
    /// snapshot——线程拥有独立数据。
    pub fn search(&self, query: &str, options: SearchOptions) -> SearchHandle<SearchResult> {
        crate::search_async::spawn_literal_search(
            self.storage.clone(),
            self.version,
            self.config.clone(),
            query.to_string(),
            options,
        )
    }

    /// 使用默认选项启动大小写敏感的全文 literal 搜索。
    pub fn search_literal(&self, query: &str) -> SearchHandle<SearchResult> {
        self.search(query, SearchOptions::default())
    }

    /// 在该不可变快照中启动一次 regex 搜索，结果绑定快照版本。
    pub fn search_regex(
        &self,
        pattern: &str,
        options: RegexSearchOptions,
    ) -> SearchHandle<RegexSearchResult> {
        crate::search_async::spawn_regex_search(
            self.storage.clone(),
            self.version,
            pattern.to_string(),
            options,
        )
    }
}
