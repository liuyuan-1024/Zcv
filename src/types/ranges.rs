//! 范围强类型：维护 TextRange 与 LineRange 的半开区间不变量。
//!
//! 构造器负责拒绝反向范围，避免恢复 public unchecked range 构造器。

use crate::CoordinateError;

use super::{CharOffset, Line};

/// 文本区间。
///
/// 由 CharOffset 构成，满足 `start <= end`，表达编辑语义区间，不是 UTF-8 字节区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextRange {
    start: CharOffset,
    end: CharOffset,
}

impl TextRange {
    /// 创建文本区间。
    ///
    /// 该构造函数会校验 `start <= end`，避免在公共 API 边界 panic。
    pub fn new(start: CharOffset, end: CharOffset) -> Result<Self, CoordinateError> {
        if start > end {
            return Err(CoordinateError::InvalidRange { start, end });
        }

        Ok(Self { start, end })
    }

    pub const fn start(self) -> CharOffset {
        self.start
    }

    pub const fn end(self) -> CharOffset {
        self.end
    }

    pub fn len(self) -> usize {
        self.end.get() - self.start.get()
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// 行区间。
///
/// `LineRange` 使用半开区间 `[start, end)` 表达一组逻辑行，满足 `start <= end`。
/// 它只表达行号范围本身，是否落在具体 Buffer 内由查询入口校验。
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
