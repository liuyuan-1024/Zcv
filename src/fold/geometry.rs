//! Fold / Projection 共用的最小行几何抽象。
//!
//! 同时给 `Buffer` 和 `Snapshot` 实现，让 fold 集合（M13A）和投影构建（M13B 起）能共享
//! 「fold range -> 行号区间」的纯几何计算，避免重复实现。
//!
//! 本 trait 仅在 crate 内可见，不进入 public API；两侧都已经在 public API 上暴露了同样的
//! 行号 / line_start / char_to_position / len_chars 入口。

use crate::{
    EngineResult,
    buffer::Buffer,
    snapshot::Snapshot,
    types::{CharOffset, Line, LineRange, Position, TextRange},
};

/// 提供 fold 与 projection 计算必需的最小行几何能力。
pub(crate) trait LineGeometry {
    fn line_count(&self) -> usize;
    fn line_start(&self, line: Line) -> EngineResult<CharOffset>;
    fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position>;
    fn len_chars(&self) -> CharOffset;
}

impl LineGeometry for Buffer {
    fn line_count(&self) -> usize {
        Buffer::line_count(self)
    }

    fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        Buffer::line_start(self, line)
    }

    fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position> {
        Buffer::char_to_position(self, offset)
    }

    fn len_chars(&self) -> CharOffset {
        Buffer::len_chars(self)
    }
}

impl LineGeometry for Snapshot {
    fn line_count(&self) -> usize {
        Snapshot::line_count(self)
    }

    fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        Snapshot::line_start(self, line)
    }

    fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position> {
        Snapshot::char_to_position(self, offset)
    }

    fn len_chars(&self) -> CharOffset {
        Snapshot::len_chars(self)
    }
}

/// 把 LineRange 翻译成对应的 char range。半开 LineRange `[start, end)` 中 `end` 允许等于
/// `line_count`（表示文档末尾）。
pub(crate) fn char_range_for_line_range<G: LineGeometry>(
    geom: &G,
    line_range: LineRange,
) -> EngineResult<TextRange> {
    let start = line_boundary_offset(geom, line_range.start())?;
    let end = line_boundary_offset(geom, line_range.end())?;
    Ok(TextRange::new(start, end)?)
}

/// 计算 line 在文本中的起始 char offset；当 line == line_count 时返回 `len_chars`。
pub(crate) fn line_boundary_offset<G: LineGeometry>(
    geom: &G,
    line: Line,
) -> EngineResult<CharOffset> {
    let line_value = line.get();
    let line_count = geom.line_count();

    if line_value > line_count {
        return Err(crate::CoordinateError::LineOutOfBounds(line).into());
    }

    if line_value == line_count {
        return Ok(geom.len_chars());
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
    let start_line = geom.char_to_position(range.start())?.line();
    let end_line = if range.is_empty() {
        start_line
    } else {
        let end_position = geom.char_to_position(range.end())?;
        if end_position.column().get() == 0 && end_position.line() > start_line {
            previous_line(end_position.line())
        } else {
            end_position.line()
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
