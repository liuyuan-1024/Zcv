//! Editor Projection 中单条投影行的类型：可见文本行 + 折叠占位符行。
//!
//! `ProjectedLine` 把行的索引与种类捆绑成一个值；`ProjectedLineKind` 区分
//! 「该行展示的是某条逻辑行」还是「该行是一段被折叠隐藏的占位符」。
//! 占位符样式（如 `…`）和绘制属于宿主，引擎只承诺数学事实。

use zcv_engine::{Line, LineRange};

use super::ProjectedLineIndex;

/// 投影行的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectedLineKind {
    /// 投影行展示的是某条可见逻辑行。
    Text(TextLine),
    /// 投影行是一段被折叠隐藏内容的占位符。
    Placeholder(FoldPlaceholder),
}

impl ProjectedLineKind {
    pub fn is_placeholder(&self) -> bool {
        matches!(self, Self::Placeholder(_))
    }

    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }

    pub fn text_line(&self) -> Option<TextLine> {
        match self {
            Self::Text(text_line) => Some(*text_line),
            Self::Placeholder(_) => None,
        }
    }

    pub fn placeholder(&self) -> Option<FoldPlaceholder> {
        match self {
            Self::Placeholder(placeholder) => Some(*placeholder),
            Self::Text(_) => None,
        }
    }
}

/// Projection 中携带索引信息的投影行视图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectedLine {
    index: ProjectedLineIndex,
    kind: ProjectedLineKind,
}

impl ProjectedLine {
    pub(super) fn new(index: ProjectedLineIndex, kind: ProjectedLineKind) -> Self {
        Self { index, kind }
    }

    pub fn index(self) -> ProjectedLineIndex {
        self.index
    }

    pub fn kind(self) -> ProjectedLineKind {
        self.kind
    }

    pub fn is_placeholder(self) -> bool {
        self.kind.is_placeholder()
    }

    pub fn is_text(self) -> bool {
        self.kind.is_text()
    }
}

/// 可见逻辑行投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextLine {
    logical_line: Line,
}

impl TextLine {
    pub fn new(logical_line: Line) -> Self {
        Self { logical_line }
    }

    pub fn logical_line(self) -> Line {
        self.logical_line
    }
}

/// 折叠占位符投影行。
///
/// `anchor_line` 是该占位符紧跟其后的可见 anchor 逻辑行；`hidden_lines` 是被折叠隐藏的
/// 半开行区间 `[first_hidden, end_exclusive)`。当多个 fold 折叠的逻辑行区间合并为一段连续
/// 的隐藏区间时，引擎只产出一条占位符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FoldPlaceholder {
    anchor_line: Line,
    hidden_lines: LineRange,
}

impl FoldPlaceholder {
    pub(super) fn new(anchor_line: Line, hidden_lines: LineRange) -> Self {
        Self {
            anchor_line,
            hidden_lines,
        }
    }

    pub fn anchor_line(self) -> Line {
        self.anchor_line
    }

    pub fn hidden_lines(self) -> LineRange {
        self.hidden_lines
    }

    pub fn hidden_line_count(self) -> usize {
        self.hidden_lines.len()
    }
}

/// 逻辑行 -> 投影空间的查询结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogicalProjection {
    /// 逻辑行可见，对应投影行索引。
    Visible(ProjectedLineIndex),
    /// 逻辑行被某段 fold 隐藏；返回该 fold 的 anchor 信息。
    Hidden {
        /// 隐藏该逻辑行的 fold anchor（该 fold 第一条仍可见的逻辑行）。
        anchor_logical_line: Line,
        /// anchor 在投影空间的索引；可作为「跳到 fold 起点」的目标。
        anchor_projected_line: ProjectedLineIndex,
    },
}

impl LogicalProjection {
    pub fn is_hidden(&self) -> bool {
        matches!(self, Self::Hidden { .. })
    }

    pub fn is_visible(&self) -> bool {
        matches!(self, Self::Visible(_))
    }

    pub fn projected_line(&self) -> ProjectedLineIndex {
        match self {
            Self::Visible(index) => *index,
            Self::Hidden {
                anchor_projected_line,
                ..
            } => *anchor_projected_line,
        }
    }
}
