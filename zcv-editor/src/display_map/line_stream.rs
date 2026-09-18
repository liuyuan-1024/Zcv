//! 显示消费链的 Buffer 行输入。
//!
//! MultiBuffer 已将工作区文本、deleted hunk 等来源统一物化为普通文本；
//! fold/tab/wrap 只消费这一份组合快照，不再维护第二套合成行坐标。

use zcv_multi_buffer::MultiBufferOffset;

use std::borrow::Cow;
use std::ops::Range;

use zcv_multi_buffer::MultiBufferSnapshot;
use zcv_text::Line;

/// 显示输入行的文本来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamLineSource(usize);

impl StreamLineSource {
    pub(crate) const fn new(line: usize) -> Self {
        Self(line)
    }

    pub(crate) const fn line(self) -> usize {
        self.0
    }
}

/// display 消费链的输入流。
#[derive(Debug, Clone)]
pub(crate) struct LineStream {
    buffer: MultiBufferSnapshot,
}

impl LineStream {
    pub(crate) fn new<S: Into<MultiBufferSnapshot>>(buffer: S) -> Self {
        let buffer = buffer.into();
        Self { buffer }
    }

    pub(crate) fn line_count(&self) -> usize {
        self.buffer.line_count()
    }

    pub(crate) fn source(&self, line: Line) -> Option<StreamLineSource> {
        let line = line.get();
        (line < self.buffer.line_count()).then_some(StreamLineSource::new(line))
    }

    pub(crate) fn buffer_to_stream(&self, line: Line) -> Line {
        line
    }

    pub(crate) fn line_text(&self, line: Line) -> Option<Cow<'_, str>> {
        let range = self.line_byte_range(line)?;
        // 空行（含末尾空行）的范围为空，没有 chunk 可消费；它仍然是有效行，文本为空串。
        if range.is_empty() {
            return Some(Cow::Borrowed(""));
        }
        let mut chunks = self.buffer.bytes_in_range(range);
        let first = chunks.next()?;
        if let Some(second) = chunks.next() {
            let mut text = String::from(first.text);
            text.push_str(second.text);
            text.extend(chunks.map(|chunk| chunk.text));
            Some(Cow::Owned(text))
        } else {
            Some(Cow::Borrowed(first.text))
        }
    }

    pub(crate) fn line_byte_range(&self, line: Line) -> Option<Range<MultiBufferOffset>> {
        let buffer_line = self.source(line)?.line();
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

    /// 虚拟组合快照（高亮、选区和行流共享唯一 source 投影）。
    pub(crate) fn buffer_snapshot(&self) -> &MultiBufferSnapshot {
        &self.buffer
    }
}
