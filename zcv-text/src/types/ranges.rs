//! 范围强类型：维护 TextRange 与 LineRange 的半开区间不变量。
//!
//! **坐标系唯一真理**：`TextRange` 由 `ByteOffset` 构成，是文本内核核心区间类型；
//! `LineRange` 仍然按逻辑行号表达，但它是边界投影（只在边界 / 公共 API 出现）。

use super::{ByteOffset, Line};
use crate::CoordinateError;

/// 文本区间 —— 文本内核核心区间类型。
///
/// 由 `ByteOffset` 构成，满足 `start <= end`，表达 UTF-8 字节区间。
/// 文本内核内部所有 Edit / ChangeSet / PositionMap / Anchor 区间都使用本类型。
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
#[path = "test/ranges_tests.rs"]
mod tests;
