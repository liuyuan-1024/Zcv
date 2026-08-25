//! Snapshot public API：提供绑定 BufferVersion 的不可变只读文本视图和坐标查询能力。
//!
//! 本文件保证后台读取可脱离可变 Buffer；它不提交编辑、不维护历史，也不暴露 Ropey 内部类型。

use crate::{
    BufferConfig, BufferVersion, ByteOffset, Line, LineContent, LineRange, LineSlice,
    RegexSearchOptions, RegexSearchResult, SearchOptions, SearchResult, TextRange, TextResult,
    TextSlice,
    search::{search_in_text, search_regex_in_text},
    slicing::{
        line_content_for_text, text_range_for_byte_range, text_range_for_line,
        text_range_for_line_range,
    },
    storage::{RopeySnapshot, TextRead, text_coordinate_gateway},
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

    /// 在该不可变快照中执行 literal 搜索，结果绑定快照版本。
    ///
    /// 本方法只执行同步匹配；后台调度、取消和进度由宿主搜索层负责。
    pub fn search(&self, query: &str, options: SearchOptions) -> TextResult<SearchResult> {
        search_in_text(&self.storage, self.version, &self.config, query, options)
    }

    /// 使用默认选项执行大小写敏感的全文 literal 搜索。
    pub fn search_literal(&self, query: &str) -> TextResult<SearchResult> {
        self.search(query, SearchOptions::default())
    }

    /// 在该不可变快照中执行 regex 搜索，结果绑定快照版本。
    pub fn search_regex(
        &self,
        pattern: &str,
        options: RegexSearchOptions,
    ) -> TextResult<RegexSearchResult> {
        search_regex_in_text(&self.storage, self.version, pattern, options)
    }
}
