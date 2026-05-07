//! 二维位置强类型：表达逻辑行列、显示列和 UTF-16 行列坐标。
//!
//! 本文件只定义坐标载体；具体转换依赖 Buffer/Snapshot 的文本内容和配置策略。

use super::Utf16Offset;

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
/// M3.5 起，逻辑列按 Unicode Scalar Value 计数，与 CharOffset 的行内单位一致。
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

/// 视觉列号，0-indexed。
///
/// 表示考虑 Tab 展开、CJK 宽度、emoji 宽度等策略后的显示列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DisplayColumn(usize);

impl DisplayColumn {
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

/// UTF-16 行列位置。
///
/// 主要用于 LSP 等使用 UTF-16 code unit 作为行内坐标的外部协议。
/// `line` 仍然是 0-indexed 逻辑行号，`character` 是该行内 UTF-16 code unit 偏移。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Utf16Position {
    /// 0-indexed 逻辑行号。
    pub line: Line,
    /// 行内 UTF-16 code unit 偏移。
    pub character: Utf16Offset,
}

impl Utf16Position {
    pub const ZERO: Self = Self {
        line: Line::ZERO,
        character: Utf16Offset::ZERO,
    };

    pub const fn new(line: Line, character: Utf16Offset) -> Self {
        Self { line, character }
    }

    pub const fn line(self) -> Line {
        self.line
    }

    pub const fn character(self) -> Utf16Offset {
        self.character
    }
}
