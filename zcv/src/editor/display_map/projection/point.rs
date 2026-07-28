//! Editor Projection point 与映射结果类型。
//!
//! `LogicalPoint` / `ProjectedPoint` 分别是逻辑文档与投影空间内的 (line, column) 强类型；
//! 与 `Position` 的区别在于显式区分两套行号语义，避免坐标混用。
//! 映射结果用 typed enum 表达可见 / 隐藏（逻辑→投影）和 Text / Placeholder（投影→逻辑），
//! 把「fold anchor 行 / hidden 行 / placeholder 行」三个事实直接暴露给调用方。

use zcv_engine::{Line, LineRange, LogicalColumn, Position};

use super::ProjectedLineIndex;

/// 逻辑文档内的 (line, column) 点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct LogicalPoint {
    pub line: Line,
    pub column: LogicalColumn,
}

impl LogicalPoint {
    pub const fn new(line: Line, column: LogicalColumn) -> Self {
        Self { line, column }
    }

    pub const fn line_start(line: Line) -> Self {
        Self {
            line,
            column: LogicalColumn::ZERO,
        }
    }

    pub const fn line(self) -> Line {
        self.line
    }

    pub const fn column(self) -> LogicalColumn {
        self.column
    }

    pub fn into_position(self) -> Position {
        Position::new(self.line, self.column)
    }
}

impl From<Position> for LogicalPoint {
    fn from(position: Position) -> Self {
        Self {
            line: position.line(),
            column: position.column(),
        }
    }
}

impl From<LogicalPoint> for Position {
    fn from(point: LogicalPoint) -> Self {
        point.into_position()
    }
}

/// 投影空间内的 (projected_line, column) 点。
///
/// 当 `line` 指向一条 `TextLine` 时 `column` 与对应逻辑行的 `LogicalColumn` 同义；
/// 当 `line` 指向一条 `FoldPlaceholder` 时 `column` 没有逻辑文本意义，由宿主决定如何使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ProjectedPoint {
    pub line: ProjectedLineIndex,
    pub column: LogicalColumn,
}

impl ProjectedPoint {
    pub const fn new(line: ProjectedLineIndex, column: LogicalColumn) -> Self {
        Self { line, column }
    }

    pub const fn line_start(line: ProjectedLineIndex) -> Self {
        Self {
            line,
            column: LogicalColumn::ZERO,
        }
    }

    pub const fn line(self) -> ProjectedLineIndex {
        self.line
    }

    pub const fn column(self) -> LogicalColumn {
        self.column
    }
}

/// `LogicalPoint` -> 投影空间的查询结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogicalPointProjection {
    /// 逻辑点所在行可见，直接对应一个投影点。
    Visible(ProjectedPoint),
    /// 逻辑点所在行被某段 fold 隐藏；返回 fold anchor 的逻辑点与投影点。
    Hidden {
        anchor_logical: LogicalPoint,
        anchor_projected: ProjectedPoint,
    },
}

impl LogicalPointProjection {
    pub fn is_hidden(&self) -> bool {
        matches!(self, Self::Hidden { .. })
    }

    pub fn is_visible(&self) -> bool {
        matches!(self, Self::Visible(_))
    }

    /// 返回该逻辑点最终落在投影空间的 `ProjectedPoint`：可见行返回自身，隐藏行返回 fold anchor。
    pub fn projected_point(&self) -> ProjectedPoint {
        match self {
            Self::Visible(point) => *point,
            Self::Hidden {
                anchor_projected, ..
            } => *anchor_projected,
        }
    }
}

/// `ProjectedPoint` -> 逻辑空间的查询结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectedPointMapping {
    /// 投影点所在行是 `TextLine`，对应单一逻辑点。
    Text(LogicalPoint),
    /// 投影点所在行是 `FoldPlaceholder`，返回 fold anchor 与该 placeholder 覆盖的隐藏行区间。
    Placeholder {
        anchor: LogicalPoint,
        hidden_lines: LineRange,
    },
}

impl ProjectedPointMapping {
    pub fn is_placeholder(&self) -> bool {
        matches!(self, Self::Placeholder { .. })
    }

    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }

    /// 返回该投影点对应的逻辑点：text 行返回自身，placeholder 行返回 fold anchor。
    pub fn logical_point(&self) -> LogicalPoint {
        match self {
            Self::Text(point) => *point,
            Self::Placeholder { anchor, .. } => *anchor,
        }
    }
}
