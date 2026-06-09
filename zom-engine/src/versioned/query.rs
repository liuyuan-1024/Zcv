//! 区间查询数学：把 `TextRange`、`LineRange` 和 offset 查询统一为半开区间相交判断。
//!
//! 无状态 helper，供 `VersionedRangeSet` 与按 kind 索引的 `MetadataLayers` 复用。

use crate::{
    buffer::Buffer,
    errors::CoordinateError,
    types::{ByteOffset, Line, LineRange, TextRange},
};

pub(crate) fn ranges_intersect(left: TextRange, right: TextRange) -> bool {
    match (left.is_empty(), right.is_empty()) {
        (true, true) => left.start() == right.start(),
        (true, false) => right.start() <= left.start() && left.start() < right.end(),
        (false, true) => left.start() <= right.start() && right.start() < left.end(),
        (false, false) => left.start() < right.end() && right.start() < left.end(),
    }
}

pub(crate) fn range_contains_offset(range: TextRange, offset: ByteOffset) -> bool {
    if range.is_empty() {
        return range.start() == offset;
    }

    range.start() <= offset && offset < range.end()
}

pub(crate) fn text_range_for_line_range(
    buffer: &Buffer,
    line_range: LineRange,
) -> crate::EngineResult<TextRange> {
    let start = byte_offset_for_line_boundary(buffer, line_range.start())?;
    let end = byte_offset_for_line_boundary(buffer, line_range.end())?;
    Ok(TextRange::new(start, end)?)
}

fn byte_offset_for_line_boundary(buffer: &Buffer, line: Line) -> crate::EngineResult<ByteOffset> {
    let line_value = line.get();
    let line_count = buffer.line_count();

    if line_value > line_count {
        return Err(CoordinateError::LineOutOfBounds(line).into());
    }

    if line_value == line_count {
        return Ok(buffer.len_bytes());
    }

    buffer.line_start_byte(line)
}
