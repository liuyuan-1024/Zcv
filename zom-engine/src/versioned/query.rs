//! 区间查询数学：把 `TextRange`、`LineRange` 和 offset 查询统一为半开区间相交判断。
//!
//! 无状态 helper，供 `VersionedRangeSet` 复用。

use crate::types::{ByteOffset, TextRange};

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

// `text_range_for_line_range` 统一走 `Buffer::text_range_for_line_range()`，
// 避免与 `crate::slicing::text_range_for_line_range` 跨模块重复实现。
// 详见 `Buffer::text_range_for_line_range` → `crate::slicing::text_range_for_line_range`。
