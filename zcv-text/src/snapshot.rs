//! Snapshot public API：提供绑定 BufferVersion 的不可变只读文本视图和坐标查询能力。
//!
//! 本文件保证后台读取可脱离可变 Buffer；它不提交编辑、不维护历史，也不暴露 Ropey 内部类型。

use crate::{
    Affinity, Anchor, BufferConfig, BufferVersion, ByteOffset, CharOffset, Line, LineRange,
    MovementDirection, MovementUnit, RegexSearchResult, SearchResult, TextChangeBatch, TextRange,
    TextResult,
    search::{
        RegexSearchOptions, SearchOptions, search_in_text, search_regex_in_text,
        search_regex_in_text_with_automata,
    },
    slicing::{LineContent, LineSlice, TextSlice},
    slicing::{
        line_content_for_text, text_range_for_byte_range, text_range_for_line,
        text_range_for_line_range,
    },
    storage::{RopeySnapshot, TextRead, text_coordinate_gateway},
    tracking::EditLog,
};

/// 不可变文本快照。
#[derive(Debug, Clone)]
pub struct Snapshot {
    storage: RopeySnapshot,
    version: BufferVersion,
    config: BufferConfig,
    /// Buffer 在该版本时可见的版本化编辑日志，供 edits_since / Anchor 跨版本解析。
    edit_log: EditLog,
}

impl Snapshot {
    pub(crate) fn new(
        storage: RopeySnapshot,
        version: BufferVersion,
        config: BufferConfig,
        edit_log: EditLog,
    ) -> Self {
        Self {
            storage,
            version,
            config,
            edit_log,
        }
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    /// 返回自 `since` 版本到本快照版本的净编辑批次。
    ///
    /// 这是 Zed BufferSnapshot::edits_since_in_range 的本地等价：
    /// Buffer 是版本化编辑的唯一事实，组合文档不再依赖订阅的瞬时批次。
    pub fn edits_since(&self, since: BufferVersion) -> TextResult<TextChangeBatch> {
        self.edit_log.batch_since(since, self.version)
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
        Anchor::new(self.version, offset).with_affinity(Affinity::Before)
    }

    /// 在 `offset` 处创建吸附到插入文本之后的锚点。
    pub fn anchor_after(&self, offset: ByteOffset) -> Anchor {
        Anchor::new(self.version, offset).with_affinity(Affinity::After)
    }

    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    /// 按纯文本粒度查找相邻边界。
    ///
    /// 快照与可编辑 Buffer 共享同一套移动语义，使显示层可以在不可变文本视图上完成显示坐标到逻辑坐标的完整移动，不必重新取得可变 Buffer。
    pub fn movement_boundary(
        &self,
        offset: CharOffset,
        direction: MovementDirection,
        unit: MovementUnit,
    ) -> TextResult<CharOffset> {
        crate::buffer::movement_boundary_in_text(
            &self.storage,
            self.config.word_boundary,
            offset,
            direction,
            unit,
        )
    }

    pub fn surrounding_word(&self, offset: CharOffset) -> TextResult<(CharOffset, CharOffset)> {
        crate::buffer::surrounding_word_in_text(&self.storage, self.config.word_boundary, offset)
    }

    pub fn is_inside_word(&self, offset: CharOffset) -> TextResult<bool> {
        crate::buffer::is_inside_word_in_text(&self.storage, self.config.word_boundary, offset)
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

    pub(crate) fn search_regex_with_automata(
        &self,
        pattern: &str,
        regex: &regex_automata::meta::Regex,
        options: RegexSearchOptions,
    ) -> TextResult<RegexSearchResult> {
        search_regex_in_text_with_automata(&self.storage, self.version, pattern, regex, options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Buffer, BufferConfig, Edit, TransactionMetadata};

    #[test]
    fn snapshot_coordinates_define_empty_document_and_eof_boundaries() {
        let empty =
            Buffer::scratch(String::new(), BufferConfig::default()).expect("空文档快照应能创建");
        let empty_snapshot = empty.snapshot();
        assert_eq!(empty_snapshot.len_bytes(), ByteOffset::ZERO);
        assert_eq!(empty_snapshot.line_count(), 1);
        assert_eq!(
            empty_snapshot.line_start_byte(Line::ZERO).unwrap(),
            ByteOffset::ZERO
        );
        assert!(empty_snapshot.line_start_byte(Line::new(1)).is_err());
        assert_eq!(
            empty_snapshot.byte_to_line(ByteOffset::ZERO).unwrap(),
            Line::ZERO
        );

        let text =
            Buffer::scratch("a\n".to_owned(), BufferConfig::default()).expect("带换行文本应能创建");
        let snapshot = text.snapshot();
        assert_eq!(snapshot.line_count(), 2);
        assert_eq!(
            snapshot.line_start_byte(Line::new(1)).unwrap(),
            ByteOffset::new(2)
        );
        assert_eq!(
            snapshot.byte_to_line(snapshot.len_bytes()).unwrap(),
            Line::new(1)
        );
        assert!(snapshot.line_start_byte(Line::new(2)).is_err());
    }

    #[test]
    fn snapshot_remains_immutable_when_buffer_advances() {
        let mut buffer =
            Buffer::scratch("a".to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
        let before = buffer.snapshot();

        buffer
            .edit(
                [Edit::insert(ByteOffset::new(1), "b").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");
        let after = buffer.snapshot();

        assert_ne!(before.version(), after.version());
        assert_eq!(before.len_bytes(), ByteOffset::new(1));
        assert_eq!(after.len_bytes(), ByteOffset::new(2));
        assert_eq!(
            before
                .slice_byte_range(ByteOffset::ZERO, before.len_bytes())
                .unwrap()
                .as_str(),
            "a"
        );
        assert_eq!(
            after
                .slice_byte_range(ByteOffset::ZERO, after.len_bytes())
                .unwrap()
                .as_str(),
            "ab"
        );
    }
}
