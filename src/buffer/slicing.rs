//! Buffer 文本切片读取入口。
//!
//! M11 只提供 Buffer 当前版本上的只读读取能力，不参与编辑、历史或折叠投影。

use crate::{
    ByteOffset, EngineResult, Line, LineRange, LineSlice, TextRange, TextSlice, Viewport,
    ViewportSlice,
    slicing::{
        text_range_for_byte_range, text_range_for_line, text_range_for_line_range,
        viewport_slice_for_text,
    },
    storage::TextRead,
};

use super::Buffer;

impl Buffer {
    /// 按 char range 读取文本。
    pub fn slice_text(&self, range: TextRange) -> EngineResult<TextSlice<'_>> {
        Ok(TextSlice::new(range, self.storage.slice_text(range)?))
    }

    /// 按 UTF-8 byte range 读取文本，主要用于文件 / 外部协议适配边界。
    pub fn slice_byte_range(
        &self,
        start: ByteOffset,
        end: ByteOffset,
    ) -> EngineResult<TextSlice<'_>> {
        let range = text_range_for_byte_range(&self.storage, start, end)?;
        self.slice_text(range)
    }

    /// 读取单个逻辑行；如果该行有换行符，返回文本会保留换行符。
    pub fn slice_line(&self, line: Line) -> EngineResult<LineSlice<'_>> {
        let range = text_range_for_line(&self.storage, line)?;
        Ok(LineSlice::new(line, self.slice_text(range)?))
    }

    /// 按半开逻辑行区间读取文本。
    pub fn slice_line_range(&self, line_range: LineRange) -> EngineResult<TextSlice<'_>> {
        let range = text_range_for_line_range(&self.storage, line_range)?;
        self.slice_text(range)
    }

    /// 按逻辑行 viewport 读取可见行。
    pub fn slice_viewport(&self, viewport: Viewport) -> EngineResult<ViewportSlice<'_>> {
        viewport_slice_for_text(&self.storage, viewport)
    }
}
