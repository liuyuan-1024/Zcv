//! 范围强类型：维护 TextRange 与 LineRange 的半开区间不变量。
//!
//! **坐标系唯一真理**：`TextRange` 由 `ByteOffset` 构成，是引擎核心区间类型；
//! `LineRange` 仍然按逻辑行号表达，但它是边界投影（只在边界 / 公共 API 出现）。

use super::{ByteOffset, Line};
use crate::CoordinateError;

/// 文本区间 —— 引擎核心区间类型。
///
/// 由 `ByteOffset` 构成，满足 `start <= end`，表达 UTF-8 字节区间。
/// 引擎内部所有 Edit / ChangeSet / PositionMap / Anchor 区间都使用本类型。
///
/// 注意：调用方有责任保证 `start` / `end` 都落在 UTF-8 字符边界上；
/// 存储后端在 `validate` 阶段会拒绝落在多字节序列中间的区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextRange {
    start: ByteOffset,
    end: ByteOffset,
}

impl TextRange {
    /// 校验 `start <= end`，避免在公共 API 边界 panic。
    pub fn new(start: ByteOffset, end: ByteOffset) -> Result<Self, CoordinateError> {
        if start > end {
            return Err(CoordinateError::InvalidRange { start, end });
        }

        Ok(Self { start, end })
    }

    pub const fn start(self) -> ByteOffset {
        self.start
    }

    pub const fn end(self) -> ByteOffset {
        self.end
    }

    /// 字节长度。
    pub fn len(self) -> usize {
        self.end.get() - self.start.get()
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// 与另一区间是否有重叠（半开区间相交）。
    pub fn overlaps(self, other: TextRange) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// `point` 是否落在 `[start, end)` 内。
    pub fn contains(self, point: ByteOffset) -> bool {
        self.start <= point && point < self.end
    }
}

/// 行区间 —— 边界投影类型。
///
/// `LineRange` 使用半开区间 `[start, end)` 表达一组逻辑行；满足 `start <= end`。
/// 仅在公共 API 边界、视图 / 投影层使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineRange {
    start: Line,
    end: Line,
}

impl LineRange {
    pub fn new(start: Line, end: Line) -> Result<Self, CoordinateError> {
        if start > end {
            return Err(CoordinateError::InvalidLineRange { start, end });
        }

        Ok(Self { start, end })
    }

    pub const fn start(self) -> Line {
        self.start
    }

    pub const fn end(self) -> Line {
        self.end
    }

    pub fn len(self) -> usize {
        self.end.get() - self.start.get()
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(value: usize) -> ByteOffset {
        ByteOffset::new(value)
    }

    fn line(value: usize) -> Line {
        Line::new(value)
    }

    #[test]
    fn text_range_reversed_byte_offsets_should_return_invalid_range() {
        let err = TextRange::new(b(8), b(3)).unwrap_err();

        assert!(matches!(
            err,
            CoordinateError::InvalidRange { start, end } if start == b(8) && end == b(3)
        ));
    }

    #[test]
    fn text_range_half_open_boundary_should_report_len_empty_overlap_and_contains() {
        let empty = TextRange::new(b(4), b(4)).unwrap();
        let left = TextRange::new(b(2), b(5)).unwrap();
        let adjacent = TextRange::new(b(5), b(9)).unwrap();
        let overlapping = TextRange::new(b(4), b(8)).unwrap();

        assert!(empty.is_empty());
        assert_eq!(left.len(), 3);
        assert!(left.contains(b(2)));
        assert!(!left.contains(b(5)));
        assert!(!left.overlaps(adjacent));
        assert!(left.overlaps(overlapping));
    }

    #[test]
    fn line_range_reversed_lines_should_return_invalid_line_range() {
        let err = LineRange::new(line(4), line(1)).unwrap_err();

        assert!(matches!(
            err,
            CoordinateError::InvalidLineRange { start, end } if start == line(4) && end == line(1)
        ));
    }
}
