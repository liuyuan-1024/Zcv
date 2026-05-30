//! Fold / Projection 共用的最小行几何抽象。
//!
//! 同时给 `Buffer` 和 `Snapshot` 实现，让 fold 集合与投影构建共享
//! 「fold range -> 行号区间」的纯几何计算，避免重复实现。
//!
//! 本 trait 仅在 crate 内可见，不进入 public API；两侧都已经在 public API 上暴露了同样的
//! 行号 / line_start / byte_to_position / len_bytes 入口。
//!
//! **深核 byte**：fold 几何计算以 `ByteOffset` 为唯一坐标。

use crate::{
    EngineResult,
    buffer::Buffer,
    snapshot::Snapshot,
    types::{ByteOffset, Line, LineRange, Position, TextRange},
};

/// 提供 fold 与 projection 计算必需的最小行几何能力（深核 byte）。
pub(crate) trait LineGeometry {
    fn line_count(&self) -> usize;
    fn line_start(&self, line: Line) -> EngineResult<ByteOffset>;
    fn byte_to_position(&self, offset: ByteOffset) -> EngineResult<Position>;
    /// 仅需行号的快路径：省掉列计算的一连串 O(log N)。
    /// 默认走 `byte_to_position`；后端 override 时直接调底层 `byte_to_line`。
    fn byte_to_line(&self, offset: ByteOffset) -> EngineResult<Line> {
        Ok(self.byte_to_position(offset)?.line())
    }
    fn len_bytes(&self) -> ByteOffset;
}

impl LineGeometry for Buffer {
    fn line_count(&self) -> usize {
        Buffer::line_count(self)
    }

    fn line_start(&self, line: Line) -> EngineResult<ByteOffset> {
        Buffer::line_start_byte(self, line)
    }

    fn byte_to_position(&self, offset: ByteOffset) -> EngineResult<Position> {
        Buffer::byte_to_position(self, offset)
    }

    fn byte_to_line(&self, offset: ByteOffset) -> EngineResult<Line> {
        Buffer::byte_to_line(self, offset)
    }

    fn len_bytes(&self) -> ByteOffset {
        Buffer::len_bytes(self)
    }
}

impl LineGeometry for Snapshot {
    fn line_count(&self) -> usize {
        Snapshot::line_count(self)
    }

    fn line_start(&self, line: Line) -> EngineResult<ByteOffset> {
        Snapshot::line_start_byte(self, line)
    }

    fn byte_to_position(&self, offset: ByteOffset) -> EngineResult<Position> {
        Snapshot::byte_to_position(self, offset)
    }

    fn byte_to_line(&self, offset: ByteOffset) -> EngineResult<Line> {
        Snapshot::byte_to_line(self, offset)
    }

    fn len_bytes(&self) -> ByteOffset {
        Snapshot::len_bytes(self)
    }
}

/// 把 LineRange 翻译成对应的字节区间。半开 LineRange `[start, end)` 中 `end` 允许等于
/// `line_count`（表示文档末尾）。
pub(crate) fn char_range_for_line_range<G: LineGeometry>(
    geom: &G,
    line_range: LineRange,
) -> EngineResult<TextRange> {
    let start = line_boundary_offset(geom, line_range.start())?;
    let end = line_boundary_offset(geom, line_range.end())?;
    Ok(TextRange::new(start, end)?)
}

/// 计算 line 在文本中的起始 byte offset；当 line == line_count 时返回 `len_bytes`。
pub(crate) fn line_boundary_offset<G: LineGeometry>(
    geom: &G,
    line: Line,
) -> EngineResult<ByteOffset> {
    let line_value = line.get();
    let line_count = geom.line_count();

    if line_value > line_count {
        return Err(crate::CoordinateError::LineOutOfBounds(line).into());
    }

    if line_value == line_count {
        return Ok(geom.len_bytes());
    }

    geom.line_start(line)
}

/// 计算 fold range 在当前文本上覆盖的逻辑行区间 `[start_line, end_line]`（闭闭）。
///
/// 若 fold 的 end offset 恰好落在某行起点（即未消耗该行的任何字符），则该行不算入 fold 跨度。
pub(crate) fn fold_line_span<G: LineGeometry>(
    geom: &G,
    range: TextRange,
) -> EngineResult<(Line, Line)> {
    let start_line = geom.byte_to_line(range.start())?;
    let end_line = if range.is_empty() {
        start_line
    } else {
        let end_byte_line = geom.byte_to_line(range.end())?;
        // 端点落在某行起点（未消耗该行字符）时，该行不计入 fold 跨度。
        // 通过比较 `range.end()` 与该行的起始字节判断，省掉一次完整的 `byte_to_position` 列计算。
        if end_byte_line > start_line && geom.line_start(end_byte_line)? == range.end() {
            previous_line(end_byte_line)
        } else {
            end_byte_line
        }
    };
    Ok((start_line, end_line))
}

pub(crate) fn next_line(line: Line) -> Line {
    Line::new(line.get().saturating_add(1))
}

pub(crate) fn previous_line(line: Line) -> Line {
    Line::new(line.get().saturating_sub(1))
}
