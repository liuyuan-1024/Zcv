//! ProjectedViewport：基于 Projection 的折叠后视口切片。
//!
//! `ProjectedViewport` 与 `Viewport` 形态一致，只是 `start_line` 是 `ProjectedLineIndex`，
//! 表达折叠后视口中第一条投影行的位置。`ProjectedViewportSlice` 把切片结果分解为：
//! - 视口内每条投影行（`ProjectedViewportRow`），区分 TextLine（带 `VisibleLine` 文本）与
//!   FoldPlaceholder（带 `FoldPlaceholder` 信息）；
//! - 视口实际覆盖的投影行半开区间；
//! - 视口内可见的逻辑行 spans（连续逻辑行合并成 `LineRange`）；
//! - 视口内被命中的 fold placeholder 列表。

use crate::{
    EngineResult,
    slicing::VisibleLine,
    types::{Line, LineRange},
};

use super::{FoldPlaceholder, ProjectedLineIndex};

/// 折叠后视口描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectedViewport {
    start_line: ProjectedLineIndex,
    line_count: usize,
    max_line_chars: Option<usize>,
}

impl ProjectedViewport {
    pub const fn new(start_line: ProjectedLineIndex, line_count: usize) -> Self {
        Self {
            start_line,
            line_count,
            max_line_chars: None,
        }
    }

    pub const fn with_max_line_chars(mut self, max_line_chars: usize) -> Self {
        self.max_line_chars = Some(max_line_chars);
        self
    }

    pub const fn without_line_limit(mut self) -> Self {
        self.max_line_chars = None;
        self
    }

    pub const fn start_line(self) -> ProjectedLineIndex {
        self.start_line
    }

    pub const fn line_count(self) -> usize {
        self.line_count
    }

    pub const fn max_line_chars(self) -> Option<usize> {
        self.max_line_chars
    }
}

/// 折叠后视口内的单条投影行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedViewportRow<'a> {
    index: ProjectedLineIndex,
    kind: ProjectedViewportRowKind<'a>,
}

impl<'a> ProjectedViewportRow<'a> {
    pub(super) fn new(index: ProjectedLineIndex, kind: ProjectedViewportRowKind<'a>) -> Self {
        Self { index, kind }
    }

    pub fn index(&self) -> ProjectedLineIndex {
        self.index
    }

    pub fn kind(&self) -> &ProjectedViewportRowKind<'a> {
        &self.kind
    }

    pub fn into_kind(self) -> ProjectedViewportRowKind<'a> {
        self.kind
    }

    pub fn is_text(&self) -> bool {
        self.kind.is_text()
    }

    pub fn is_placeholder(&self) -> bool {
        self.kind.is_placeholder()
    }
}

/// 视口投影行内容种类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectedViewportRowKind<'a> {
    /// 可见 TextLine：携带逻辑行号与视口截断后的可见文本。
    Text {
        logical_line: Line,
        visible: VisibleLine<'a>,
    },
    /// 折叠占位符：仅携带 placeholder 元信息，渲染样式由宿主决定。
    Placeholder(FoldPlaceholder),
}

impl<'a> ProjectedViewportRowKind<'a> {
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }

    pub fn is_placeholder(&self) -> bool {
        matches!(self, Self::Placeholder(_))
    }

    pub fn logical_line(&self) -> Option<Line> {
        match self {
            Self::Text { logical_line, .. } => Some(*logical_line),
            Self::Placeholder(_) => None,
        }
    }

    pub fn placeholder(&self) -> Option<FoldPlaceholder> {
        match self {
            Self::Placeholder(placeholder) => Some(*placeholder),
            Self::Text { .. } => None,
        }
    }

    pub fn visible_line(&self) -> Option<&VisibleLine<'a>> {
        match self {
            Self::Text { visible, .. } => Some(visible),
            Self::Placeholder(_) => None,
        }
    }
}

/// 一次 ProjectedViewport 读取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedViewportSlice<'a> {
    viewport: ProjectedViewport,
    projected_line_range: ProjectedLineRange,
    rows: Vec<ProjectedViewportRow<'a>>,
    logical_line_spans: Vec<LineRange>,
    placeholders: Vec<FoldPlaceholder>,
}

impl<'a> ProjectedViewportSlice<'a> {
    pub(super) fn new(
        viewport: ProjectedViewport,
        projected_line_range: ProjectedLineRange,
        rows: Vec<ProjectedViewportRow<'a>>,
        logical_line_spans: Vec<LineRange>,
        placeholders: Vec<FoldPlaceholder>,
    ) -> Self {
        Self {
            viewport,
            projected_line_range,
            rows,
            logical_line_spans,
            placeholders,
        }
    }

    pub fn viewport(&self) -> ProjectedViewport {
        self.viewport
    }

    /// 实际覆盖的投影行半开区间 `[start, end)`。
    pub fn projected_line_range(&self) -> ProjectedLineRange {
        self.projected_line_range
    }

    pub fn rows(&self) -> &[ProjectedViewportRow<'a>] {
        &self.rows
    }

    pub fn into_rows(self) -> Vec<ProjectedViewportRow<'a>> {
        self.rows
    }

    /// 视口内涉及的逻辑行 spans；连续的可见逻辑行合并成同一条 `LineRange`。
    pub fn logical_line_spans(&self) -> &[LineRange] {
        &self.logical_line_spans
    }

    /// 视口内被命中的 fold placeholder 列表（按投影顺序）。
    pub fn placeholders(&self) -> &[FoldPlaceholder] {
        &self.placeholders
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// 投影行半开区间 `[start, end)`，表达本次切片实际命中的投影行范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectedLineRange {
    start: ProjectedLineIndex,
    end: ProjectedLineIndex,
}

impl ProjectedLineRange {
    pub(super) fn new(start: ProjectedLineIndex, end: ProjectedLineIndex) -> Self {
        debug_assert!(start.get() <= end.get(), "ProjectedLineRange 必须有序");
        Self { start, end }
    }

    pub fn start(self) -> ProjectedLineIndex {
        self.start
    }

    pub fn end(self) -> ProjectedLineIndex {
        self.end
    }

    pub fn len(self) -> usize {
        self.end.get() - self.start.get()
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// 内部 helper：合并 ProjectedViewport 切片得到的连续逻辑行 -> `LineRange` spans。
pub(super) fn build_logical_spans<'a>(
    rows: &[ProjectedViewportRow<'a>],
) -> EngineResult<Vec<LineRange>> {
    let mut spans: Vec<LineRange> = Vec::new();
    let mut current: Option<(Line, Line)> = None;

    for row in rows {
        if let Some(logical_line) = row.kind().logical_line() {
            match current {
                Some((start, end)) if logical_line.get() == end.get() => {
                    current = Some((start, Line::new(end.get() + 1)));
                }
                Some((start, end)) => {
                    spans.push(LineRange::new(start, end)?);
                    current = Some((logical_line, Line::new(logical_line.get() + 1)));
                }
                None => {
                    current = Some((logical_line, Line::new(logical_line.get() + 1)));
                }
            }
        }
    }

    if let Some((start, end)) = current {
        spans.push(LineRange::new(start, end)?);
    }

    Ok(spans)
}
