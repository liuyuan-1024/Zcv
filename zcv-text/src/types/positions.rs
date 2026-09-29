//! 二维位置强类型：表达逻辑行列坐标。
//!
//! 本文件只定义坐标载体；具体转换依赖 Buffer/Snapshot 的文本内容和配置策略。

/// 逻辑行号，0-indexed。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Line(usize);

impl Line {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

/// 逻辑列号，0-indexed。
///
/// 按 Unicode Scalar Value 计数，与 CharOffset 的行内单位一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct LogicalColumn(usize);

impl LogicalColumn {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

/// 逻辑文本位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Position {
    /// 0-indexed 逻辑行号。
    pub line: Line,
    /// 行内逻辑列，按 Unicode scalar value 计数。
    pub column: LogicalColumn,
}

impl Position {
    pub const ZERO: Self = Self {
        line: Line::ZERO,
        column: LogicalColumn::ZERO,
    };

    pub const fn new(line: Line, column: LogicalColumn) -> Self {
        Self { line, column }
    }

    pub const fn line(self) -> Line {
        self.line
    }

    pub const fn column(self) -> LogicalColumn {
        self.column
    }
}
