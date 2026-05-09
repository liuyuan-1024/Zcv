//! Projection range 类型。
//!
//! `LogicalRange` 与 `ProjectedRange` 用 `(start, end)` 强类型 point 对表达半开区间，
//! 构造器拒绝反向区间，避免恢复 unchecked range 入口。

use crate::CoordinateError;

use super::{LogicalPoint, ProjectedPoint};

/// 逻辑文档内的有序点对范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LogicalRange {
    start: LogicalPoint,
    end: LogicalPoint,
}

impl LogicalRange {
    /// 创建逻辑范围；要求 `start <= end`（按 line, column 字典序）。
    pub fn new(start: LogicalPoint, end: LogicalPoint) -> Result<Self, CoordinateError> {
        if !is_ordered_logical(start, end) {
            return Err(CoordinateError::InvalidLineRange {
                start: start.line,
                end: end.line,
            });
        }
        Ok(Self { start, end })
    }

    pub fn caret(point: LogicalPoint) -> Self {
        Self {
            start: point,
            end: point,
        }
    }

    pub const fn start(self) -> LogicalPoint {
        self.start
    }

    pub const fn end(self) -> LogicalPoint {
        self.end
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn is_single_line(self) -> bool {
        self.start.line == self.end.line
    }
}

/// 投影空间内的有序点对范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectedRange {
    start: ProjectedPoint,
    end: ProjectedPoint,
}

impl ProjectedRange {
    /// 创建投影范围；要求 `start <= end`（按 projected line, column 字典序）。
    pub fn new(start: ProjectedPoint, end: ProjectedPoint) -> Result<Self, CoordinateError> {
        if !is_ordered_projected(start, end) {
            return Err(CoordinateError::InvalidLineRange {
                start: crate::types::Line::new(start.line.get()),
                end: crate::types::Line::new(end.line.get()),
            });
        }
        Ok(Self { start, end })
    }

    pub fn caret(point: ProjectedPoint) -> Self {
        Self {
            start: point,
            end: point,
        }
    }

    pub const fn start(self) -> ProjectedPoint {
        self.start
    }

    pub const fn end(self) -> ProjectedPoint {
        self.end
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn is_single_line(self) -> bool {
        self.start.line == self.end.line
    }
}

fn is_ordered_logical(start: LogicalPoint, end: LogicalPoint) -> bool {
    if start.line < end.line {
        return true;
    }
    if start.line == end.line {
        return start.column <= end.column;
    }
    false
}

fn is_ordered_projected(start: ProjectedPoint, end: ProjectedPoint) -> bool {
    if start.line < end.line {
        return true;
    }
    if start.line == end.line {
        return start.column <= end.column;
    }
    false
}
