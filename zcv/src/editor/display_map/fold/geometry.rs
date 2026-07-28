//! Editor Fold / Projection 共用的最小行几何抽象。
//!
//! **深核 byte**：fold 几何计算以 `ByteOffset` 为唯一坐标。

use super::super::error::DisplayMapResult;
use zcv_engine::{ByteOffset, CoordinateError, Line, LineRange, Snapshot, TextRange};

/// 把 LineRange 翻译成对应的字节区间。半开 LineRange `[start, end)` 中 `end` 允许等于
/// `line_count`（表示文档末尾）。
pub(crate) fn char_range_for_line_range(
    snapshot: &Snapshot,
    line_range: LineRange,
) -> DisplayMapResult<TextRange> {
    let start = line_boundary_offset(snapshot, line_range.start())?;
    let end = line_boundary_offset(snapshot, line_range.end())?;
    Ok(TextRange::new(start, end)?)
}

/// 计算 line 在文本中的起始 byte offset；当 line == line_count 时返回 `len_bytes`。
pub(crate) fn line_boundary_offset(
    snapshot: &Snapshot,
    line: Line,
) -> DisplayMapResult<ByteOffset> {
    let line_value = line.get();
    let line_count = snapshot.line_count();

    if line_value > line_count {
        return Err(CoordinateError::LineOutOfBounds(line).into());
    }

    if line_value == line_count {
        return Ok(snapshot.len_bytes());
    }

    Ok(snapshot.line_start_byte(line)?)
}

/// 计算 fold range 在当前文本上覆盖的逻辑行区间 `[start_line, end_line]`（闭闭）。
///
/// 若 fold 的 end offset 恰好落在某行起点（即未消耗该行的任何字符），则该行不算入 fold 跨度。
pub(crate) fn fold_line_span(
    snapshot: &Snapshot,
    range: TextRange,
) -> DisplayMapResult<(Line, Line)> {
    let start_line = snapshot.byte_to_line(range.start())?;
    let end_line = if range.is_empty() {
        start_line
    } else {
        let end_byte_line = snapshot.byte_to_line(range.end())?;
        // 端点落在某行起点（未消耗该行字符）时，该行不计入 fold 跨度。
        // 通过比较 `range.end()` 与该行的起始字节判断，省掉一次完整的 `byte_to_position` 列计算。
        if end_byte_line > start_line && snapshot.line_start_byte(end_byte_line)? == range.end() {
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
