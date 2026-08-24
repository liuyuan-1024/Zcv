//! 消费链文本流：buffer 行 + 合成行（外部文本）的统一输入行空间。
//!
//! fold/tab/wrap 层只消费本流，不感知文本来源，DeletedHunk 等外部文本在流中免费获得折叠、软换行、坐标、滚动与命中测试。
//! 流在 display 链的最底层（fold 之下），因此外部文本与 buffer 文本一样可被折叠。
//!
//! 合成行自持文本（Arc<str>，如 git 删除块展开后的 HEAD 内容），不占用 buffer 字节坐标：
//! 行内偏移 = 文本字节偏移；
//! 跨行坐标映射到锚定 buffer 行的行首（与折叠区间同语义，roundtrip 不可逆）。

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use gpui::HighlightStyle;
use zcv_text::{ByteOffset, Line, Snapshot};

/// 合成行的行内样式段（字节区间 → 样式；终端等宿主注入逐格样式用）。
#[derive(Debug, Clone, PartialEq)]
pub struct StyledSpan {
    pub range: Range<usize>,
    pub style: HighlightStyle,
}

/// 合成行：自持文本 + 行内样式（git diff 等纯文本行样式为空）。
#[derive(Debug, Clone, PartialEq)]
pub struct StyledLine {
    pub text: Arc<str>,
    pub styles: Arc<[StyledSpan]>,
}

impl StyledLine {
    /// 纯文本合成行（无行内样式）。
    pub fn plain(text: Arc<str>) -> Self {
        Self {
            text,
            styles: Arc::from([]),
        }
    }
}

/// 合成行表：锚定 buffer 逻辑行 → 插入在其**之后**的文本行（自持，无行尾换行）。
///
/// 删除块展开的合成行显示在被删行的原位置（删除点 = 锚定行之后）；
/// 锚定行 0 的块插在行 0 之后。
pub(crate) type InsertedLines = BTreeMap<Line, Vec<StyledLine>>;

/// 统一行空间中一行的文本来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamLineSource {
    /// buffer 逻辑行。
    Buffer(usize),
    /// 合成行：锚定行 + 块内索引。
    Inserted { anchor: Line, index: usize },
}

/// 行文本：buffer 借用 或 合成行文本。
#[derive(Debug, Clone)]
pub(crate) enum StreamLineText<'a> {
    Buffer(std::borrow::Cow<'a, str>),
    Inserted(&'a str),
}

impl StreamLineText<'_> {
    /// 返回底层文本（借用链追溯到流，而非中间容器）。
    pub(crate) fn as_str(&self) -> &str {
        match self {
            StreamLineText::Buffer(text) => text.as_ref(),
            StreamLineText::Inserted(text) => text,
        }
    }
}

/// 统一行空间（buffer 行 + 合成行交错）：display 消费链的输入流。
#[derive(Debug, Clone)]
pub(crate) struct LineStream {
    buffer: Snapshot,
    /// 锚定 buffer 逻辑行 → 合成文本行（块插在锚定行之后）。
    inserted: InsertedLines,
    /// 合成行配置版本（与 buffer 版本区分，配置变化时递增）。
    inserted_version: u64,
}

impl LineStream {
    pub(crate) fn new(buffer: Snapshot) -> Self {
        Self {
            buffer,
            inserted: BTreeMap::new(),
            inserted_version: 0,
        }
    }

    /// 替换合成行表（版本递增；触发消费链重建）。
    pub(crate) fn set_inserted(&mut self, inserted: InsertedLines) {
        self.inserted = inserted;
        self.inserted_version += 1;
    }

    /// 合成行配置版本（buffer 之外的变化信号，供消费链判断是否结构变化）。
    pub(crate) fn inserted_version(&self) -> u64 {
        self.inserted_version
    }

    /// 统一行总数（buffer 行 + 合成行）。
    pub(crate) fn line_count(&self) -> usize {
        self.buffer.line_count() + self.total_inserted()
    }

    fn total_inserted(&self) -> usize {
        self.inserted.values().map(Vec::len).sum()
    }

    /// 统一行号 → 来源。
    pub(crate) fn source(&self, line: Line) -> Option<StreamLineSource> {
        let line = line.get();
        if line >= self.line_count() {
            return None;
        }
        // 块按锚定行序（buffer 行序）；块少，线性扫描。
        // 锚定行自身的流行号 = 锚定行号 + 其前（锚定行更早的）块的合成行数；
        // 块插在锚定行之后，起始流行号 = 锚定行流行号 + 1。
        let mut inserted_before = 0usize;
        for (anchor, lines) in &self.inserted {
            let anchor_stream = anchor.get() + inserted_before;
            let block_stream_start = anchor_stream + 1;
            if line >= block_stream_start && line < block_stream_start + lines.len() {
                return Some(StreamLineSource::Inserted {
                    anchor: *anchor,
                    index: line - block_stream_start,
                });
            }
            // 只有锚定行严格在 line 之前的块，其合成行才计入 line 之前的行数。
            if line > anchor_stream {
                inserted_before += lines.len();
            }
        }
        let buffer_line = line - inserted_before;
        if buffer_line < self.buffer.line_count() {
            Some(StreamLineSource::Buffer(buffer_line))
        } else {
            None
        }
    }

    /// buffer 逻辑行 → 流行号（锚定行严格在前的块计入前缀）。
    pub(crate) fn buffer_to_stream(&self, line: Line) -> Line {
        let before: usize = self
            .inserted
            .range(..line)
            .map(|(_, lines)| lines.len())
            .sum();
        Line::new(line.get() + before)
    }

    /// 锚定行的块起始流行号（块插在锚定行之后；无块返回 None）。
    pub(crate) fn inserted_block_start(&self, anchor: Line) -> Option<Line> {
        let before: usize = self
            .inserted
            .range(..anchor)
            .map(|(_, lines)| lines.len())
            .sum();
        self.inserted
            .get(&anchor)
            .map(|_| Line::new(anchor.get() + before + 1))
    }

    /// 行文本（统一行号；越界返回 None）。
    pub(crate) fn line_text(&self, line: Line) -> Option<StreamLineText<'_>> {
        match self.source(line)? {
            StreamLineSource::Buffer(buffer_line) => {
                let slice = self.buffer.slice_line(Line::new(buffer_line)).ok()?;
                Some(StreamLineText::Buffer(slice.into_text()))
            }
            StreamLineSource::Inserted { anchor, index } => Some(StreamLineText::Inserted(
                &self.inserted.get(&anchor)?.get(index)?.text,
            )),
        }
    }

    /// 合成行的行内样式（buffer 行为 None，语法高亮走 buffer 通道）。
    pub(crate) fn line_styles(&self, line: Line) -> Option<&[StyledSpan]> {
        match self.source(line)? {
            StreamLineSource::Buffer(_) => None,
            StreamLineSource::Inserted { anchor, index } => {
                Some(&self.inserted.get(&anchor)?.get(index)?.styles)
            }
        }
    }

    /// 行的字节范围：buffer 行 → 真实范围；合成行 → 锚定行行首的伪坐标（不可逆）。
    pub(crate) fn line_byte_range(&self, line: Line) -> Option<Range<ByteOffset>> {
        match self.source(line)? {
            StreamLineSource::Buffer(buffer_line) => {
                let start = self.buffer.line_start_byte(Line::new(buffer_line)).ok()?;
                // 行尾 = 下一行行首（或文档末尾），与 fold 的 line_boundary 同模式。
                let end = if buffer_line + 1 < self.buffer.line_count() {
                    self.buffer
                        .line_start_byte(Line::new(buffer_line + 1))
                        .ok()?
                } else {
                    self.buffer.len_bytes()
                };
                Some(start..end)
            }
            StreamLineSource::Inserted { anchor, .. } => {
                let start = self.buffer.line_start_byte(anchor).ok()?;
                Some(start..start)
            }
        }
    }

    /// buffer 快照（高亮、选区等真实 buffer 需求；行文本读取请用 `line_text`）。
    pub(crate) fn buffer_snapshot(&self) -> &Snapshot {
        &self.buffer
    }
}
