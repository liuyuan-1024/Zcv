//! Projection：基于 Snapshot + FoldSet 构建的不可变投影行映射快照。
//!
//! Projection 把「逻辑行」与「投影行」之间的双向映射固化成一组只读数组：
//! - 每条投影行要么是某条可见逻辑行（`TextLine`），要么是合并后的折叠占位符（`FoldPlaceholder`）。
//! - 每条逻辑行要么可见（指向自己的投影行），要么被某段 fold 隐藏（指向 fold anchor 的投影行）。
//!
//! 多条互相嵌套或邻接的 fold 在投影空间里只产出一条占位符（与 M13A 的 HiddenRange 合并语义保持一致）。
//! 占位符样式、像素绘制和 viewport 切片不在本类型承诺范围内。

use crate::{
    EngineResult, FoldSet,
    errors::ProjectionError,
    fold::geometry::{fold_line_span, next_line, previous_line},
    snapshot::Snapshot,
    types::{BufferVersion, Line, LineRange},
};

use super::{
    FoldPlaceholder, LogicalProjection, ProjectedLine, ProjectedLineIndex, ProjectedLineKind,
    TextLine,
};

/// 不可变投影行映射快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    version: BufferVersion,
    /// 投影行索引 -> 投影行种类。
    rows: Vec<ProjectedLineKind>,
    /// 逻辑行索引 -> 投影空间状态。长度等于 `logical_line_count`。
    logical_to_projection: Vec<LogicalProjection>,
    /// 文本来源中的逻辑行总数（不受折叠影响）。
    logical_line_count: usize,
}

impl Projection {
    /// 基于 `Snapshot` 与 `FoldSet` 构建一份新的投影。
    ///
    /// 要求两者版本一致；版本不匹配时返回 `ProjectionError::VersionMismatch`。
    pub fn build(snapshot: &Snapshot, folds: &FoldSet) -> EngineResult<Self> {
        if snapshot.version() != folds.version() {
            return Err(ProjectionError::VersionMismatch {
                snapshot_version: snapshot.version(),
                fold_version: folds.version(),
            }
            .into());
        }

        let logical_line_count = snapshot.line_count();
        let hidden_spans = collect_merged_hidden_spans(snapshot, folds)?;

        let mut rows: Vec<ProjectedLineKind> = Vec::new();
        let mut logical_to_projection: Vec<LogicalProjection> =
            Vec::with_capacity(logical_line_count);
        let mut span_cursor = 0usize;
        let mut anchor_indices: Vec<Option<ProjectedLineIndex>> = vec![None; logical_line_count];

        for line_value in 0..logical_line_count {
            let logical_line = Line::new(line_value);

            while span_cursor < hidden_spans.len() && hidden_spans[span_cursor].end <= logical_line
            {
                span_cursor += 1;
            }

            let inside_hidden = span_cursor < hidden_spans.len()
                && hidden_spans[span_cursor].contains_line(logical_line);

            if inside_hidden {
                let span = hidden_spans[span_cursor];
                let anchor_logical_line = previous_line(span.start);
                let anchor_projected_line = anchor_indices[anchor_logical_line.get()]
                    .expect("hidden span 的 anchor 必须先于隐藏行被访问");
                logical_to_projection.push(LogicalProjection::Hidden {
                    anchor_logical_line,
                    anchor_projected_line,
                });
                continue;
            }

            let projected_index = ProjectedLineIndex::new(rows.len());
            rows.push(ProjectedLineKind::Text(TextLine::new(logical_line)));
            anchor_indices[line_value] = Some(projected_index);
            logical_to_projection.push(LogicalProjection::Visible(projected_index));

            if span_cursor < hidden_spans.len() {
                let next_span = hidden_spans[span_cursor];
                if next_span.start == next_line(logical_line) {
                    let placeholder = FoldPlaceholder::new(
                        logical_line,
                        LineRange::new(next_span.start, next_span.end)?,
                    );
                    rows.push(ProjectedLineKind::Placeholder(placeholder));
                }
            }
        }

        Ok(Self {
            version: snapshot.version(),
            rows,
            logical_to_projection,
            logical_line_count,
        })
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    /// 投影行总数；等价于 `text line 数 + fold placeholder 数`。
    pub fn line_count(&self) -> usize {
        self.rows.len()
    }

    /// 来源文本的逻辑行总数。
    pub fn logical_line_count(&self) -> usize {
        self.logical_line_count
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// 按投影行索引读取行视图；越界返回 None。
    pub fn projected_line(&self, index: ProjectedLineIndex) -> Option<ProjectedLine> {
        self.rows
            .get(index.get())
            .copied()
            .map(|kind| ProjectedLine::new(index, kind))
    }

    /// 顺序遍历所有投影行（带索引）。
    pub fn iter(&self) -> impl Iterator<Item = ProjectedLine> + '_ {
        self.rows
            .iter()
            .copied()
            .enumerate()
            .map(|(idx, kind)| ProjectedLine::new(ProjectedLineIndex::new(idx), kind))
    }

    /// 投影行索引 -> 投影行种类（不带索引包裹）。
    pub fn projected_line_kind(&self, index: ProjectedLineIndex) -> Option<ProjectedLineKind> {
        self.rows.get(index.get()).copied()
    }

    /// 逻辑行 -> 投影空间状态。
    pub fn logical_to_projected(&self, line: Line) -> EngineResult<LogicalProjection> {
        self.logical_to_projection
            .get(line.get())
            .copied()
            .ok_or_else(|| crate::CoordinateError::LineOutOfBounds(line).into())
    }

    /// 判断某条逻辑行是否被任意 fold 隐藏。
    pub fn is_logical_line_hidden(&self, line: Line) -> EngineResult<bool> {
        Ok(self.logical_to_projected(line)?.is_hidden())
    }

    /// hidden 逻辑行 -> 其所在 fold 的 anchor 逻辑行。可见行返回自身。
    pub fn fold_anchor_for_logical_line(&self, line: Line) -> EngineResult<Line> {
        match self.logical_to_projected(line)? {
            LogicalProjection::Visible(_) => Ok(line),
            LogicalProjection::Hidden {
                anchor_logical_line,
                ..
            } => Ok(anchor_logical_line),
        }
    }

    /// fold placeholder 投影行 -> 该 fold 的 anchor 逻辑行；非 placeholder 返回 None。
    pub fn fold_anchor_for_projected_line(&self, index: ProjectedLineIndex) -> Option<Line> {
        self.projected_line_kind(index).and_then(|kind| match kind {
            ProjectedLineKind::Placeholder(placeholder) => Some(placeholder.anchor_line()),
            ProjectedLineKind::Text(_) => None,
        })
    }

    pub fn is_stale_for_version(&self, version: BufferVersion) -> bool {
        self.version != version
    }
}

#[derive(Debug, Clone, Copy)]
struct HiddenSpan {
    start: Line,
    end: Line,
}

impl HiddenSpan {
    fn contains_line(self, line: Line) -> bool {
        self.start <= line && line < self.end
    }
}

fn collect_merged_hidden_spans(
    snapshot: &Snapshot,
    folds: &FoldSet,
) -> EngineResult<Vec<HiddenSpan>> {
    let mut spans: Vec<HiddenSpan> = Vec::new();
    for fold in folds.iter() {
        let (start_line, end_line) = fold_line_span(snapshot, fold.range())?;
        if start_line < end_line {
            spans.push(HiddenSpan {
                start: next_line(start_line),
                end: next_line(end_line),
            });
        }
    }

    spans.sort_by_key(|span| span.start);

    let mut merged: Vec<HiddenSpan> = Vec::with_capacity(spans.len());
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.start <= last.end => {
                if span.end > last.end {
                    last.end = span.end;
                }
            }
            _ => merged.push(span),
        }
    }

    Ok(merged)
}
