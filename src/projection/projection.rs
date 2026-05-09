//! Projection：基于 Snapshot + FoldSet 构建的不可变投影行映射快照。
//!
//! Projection 把「逻辑行」与「投影行」之间的双向映射固化成一组只读数组：
//! - 每条投影行要么是某条可见逻辑行（`TextLine`），要么是合并后的折叠占位符（`FoldPlaceholder`）。
//! - 每条逻辑行要么可见（指向自己的投影行），要么被某段 fold 隐藏（指向 fold anchor 的投影行）。
//!
//! 多条互相嵌套或邻接的 fold 在投影空间里只产出一条占位符（与 M13A 的 HiddenRange 合并语义保持一致）。
//! 占位符样式、像素绘制和 viewport 切片不在本类型承诺范围内。

use crate::{
    CharOffset, EngineResult, FoldSet,
    errors::ProjectionError,
    fold::geometry::{fold_line_span, next_line, previous_line},
    slicing::{TextSlice, VisibleLine},
    snapshot::Snapshot,
    types::{BufferVersion, Line, LineRange, TextRange},
};

use super::{
    FoldPlaceholder, LogicalPoint, LogicalPointProjection, LogicalProjection, LogicalRange,
    ProjectedLine, ProjectedLineIndex, ProjectedLineKind, ProjectedLineRange, ProjectedPoint,
    ProjectedPointMapping, ProjectedRange, ProjectedViewport, ProjectedViewportRow,
    ProjectedViewportRowKind, ProjectedViewportSlice, TextLine, viewport::build_logical_spans,
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

    /// 逻辑点 -> 投影空间。可见行返回直接投影；隐藏行返回 fold anchor 的逻辑/投影点。
    pub fn logical_to_projected_point(
        &self,
        point: LogicalPoint,
    ) -> EngineResult<LogicalPointProjection> {
        match self.logical_to_projected(point.line)? {
            LogicalProjection::Visible(projected_line) => Ok(LogicalPointProjection::Visible(
                ProjectedPoint::new(projected_line, point.column),
            )),
            LogicalProjection::Hidden {
                anchor_logical_line,
                anchor_projected_line,
            } => Ok(LogicalPointProjection::Hidden {
                anchor_logical: LogicalPoint::line_start(anchor_logical_line),
                anchor_projected: ProjectedPoint::line_start(anchor_projected_line),
            }),
        }
    }

    /// 投影点 -> 逻辑空间。Text 行返回直接逻辑点；Placeholder 行返回 fold anchor 与覆盖的隐藏行区间。
    pub fn projected_to_logical_point(
        &self,
        point: ProjectedPoint,
    ) -> EngineResult<ProjectedPointMapping> {
        let kind = self
            .projected_line_kind(point.line)
            .ok_or_else(|| crate::CoordinateError::LineOutOfBounds(Line::new(point.line.get())))?;

        match kind {
            ProjectedLineKind::Text(text_line) => Ok(ProjectedPointMapping::Text(
                LogicalPoint::new(text_line.logical_line(), point.column),
            )),
            ProjectedLineKind::Placeholder(placeholder) => Ok(ProjectedPointMapping::Placeholder {
                anchor: LogicalPoint::line_start(placeholder.anchor_line()),
                hidden_lines: placeholder.hidden_lines(),
            }),
        }
    }

    /// 逻辑范围 -> 投影空间的最大连续段列表。
    ///
    /// 每条段是一条 `ProjectedRange`，对应投影空间内一段连续、且 row kind 不切换的范围：
    /// - 文本段：覆盖一个或多个连续 `TextLine` 行；
    /// - 占位符段：覆盖单个 `FoldPlaceholder` 行（多条隐藏逻辑行合并成同一条占位符时只有一段）。
    ///
    /// 段在 row kind 由 text 切换到 placeholder（或反向）时分裂；段顺序与投影空间顺序一致。
    /// 空范围返回空 Vec；端点位于 fold 隐藏区域内时，会展开到 fold anchor / 占位符的边界，
    /// 保证 selection 跨过 fold 时仍能在投影空间无歧义地高亮。
    pub fn logical_to_projected_range_segments(
        &self,
        range: LogicalRange,
    ) -> EngineResult<Vec<ProjectedRange>> {
        if range.is_empty() {
            return Ok(Vec::new());
        }

        let start_proj = match self.logical_to_projected_point(range.start())? {
            LogicalPointProjection::Visible(point) => point,
            LogicalPointProjection::Hidden {
                anchor_projected, ..
            } => anchor_projected,
        };

        let end_proj = match self.logical_to_projected_point(range.end())? {
            LogicalPointProjection::Visible(point) => point,
            LogicalPointProjection::Hidden {
                anchor_projected, ..
            } => {
                // 隐藏端点合并到该 fold 的 placeholder 行结束位置（exclusive 下一行起点）。
                let placeholder_line = ProjectedLineIndex::new(anchor_projected.line().get() + 1);
                let after_placeholder = placeholder_line.get() + 1;
                let max_line = self.line_count().saturating_sub(1);
                let end_line = ProjectedLineIndex::new(after_placeholder.min(max_line));
                ProjectedPoint::line_start(end_line)
            }
        };

        if start_proj.line() > end_proj.line()
            || (start_proj.line() == end_proj.line() && start_proj.column() > end_proj.column())
        {
            return Ok(Vec::new());
        }

        // 找出 [start_proj.line, end_proj.line] 范围内所有 row kind 切换边界，按它们切分段。
        let mut breakpoints: Vec<ProjectedPoint> = vec![start_proj];
        let mut prev_is_placeholder = self
            .projected_line_kind(start_proj.line())
            .map(|kind| kind.is_placeholder())
            .unwrap_or(false);

        for row_value in (start_proj.line().get() + 1)..=end_proj.line().get() {
            let row_idx = ProjectedLineIndex::new(row_value);
            let row_is_placeholder = self
                .projected_line_kind(row_idx)
                .map(|kind| kind.is_placeholder())
                .unwrap_or(false);
            if row_is_placeholder != prev_is_placeholder {
                breakpoints.push(ProjectedPoint::line_start(row_idx));
                prev_is_placeholder = row_is_placeholder;
            }
        }
        breakpoints.push(end_proj);

        let mut segments = Vec::with_capacity(breakpoints.len().saturating_sub(1));
        for window in breakpoints.windows(2) {
            let start = window[0];
            let end = window[1];
            if start == end {
                continue;
            }
            segments.push(ProjectedRange::new(start, end)?);
        }

        Ok(segments)
    }

    /// 投影范围 -> 逻辑范围。Placeholder 端按 `ProjectedPointMapping::Placeholder` 的 anchor 决定逻辑端点：
    /// 起点放到 fold anchor 的行起点；终点放到 placeholder 覆盖隐藏行后的第一条逻辑行起点（即 `hidden_lines.end`），
    /// 这样投影空间的「跨 placeholder 选区」就能在逻辑空间无歧义地展开为「fold anchor 起点 -> 隐藏区间结束」。
    pub fn projected_to_logical_range(&self, range: ProjectedRange) -> EngineResult<LogicalRange> {
        let start_logical = self.projected_point_to_logical_range_endpoint(range.start(), true)?;
        let end_logical = self.projected_point_to_logical_range_endpoint(range.end(), false)?;
        Ok(LogicalRange::new(start_logical, end_logical)?)
    }

    fn projected_point_to_logical_range_endpoint(
        &self,
        point: ProjectedPoint,
        is_start: bool,
    ) -> EngineResult<LogicalPoint> {
        match self.projected_to_logical_point(point)? {
            ProjectedPointMapping::Text(logical) => Ok(logical),
            ProjectedPointMapping::Placeholder {
                anchor,
                hidden_lines,
            } => {
                if is_start {
                    Ok(anchor)
                } else {
                    Ok(LogicalPoint::line_start(hidden_lines.end()))
                }
            }
        }
    }

    /// 把任意 `TextRange`（典型来源：`Selection::range()`）投影成投影空间的段列表。
    ///
    /// `snapshot` 必须与本 Projection 同版本；版本不一致返回 `ProjectionError::VersionMismatch`。
    /// 内部走 `Snapshot::char_to_position` 把端点 char offset 翻译成 `LogicalPoint`，
    /// 再调用 `logical_to_projected_range_segments`。
    /// 多 selection 投影由 caller 在循环里调用本方法即可，不需要专用入口。
    pub fn project_text_range(
        &self,
        snapshot: &Snapshot,
        range: crate::types::TextRange,
    ) -> EngineResult<Vec<ProjectedRange>> {
        self.verify_snapshot_version(snapshot)?;
        let start_position = snapshot.char_to_position(range.start())?;
        let end_position = snapshot.char_to_position(range.end())?;
        let logical_range = LogicalRange::new(
            LogicalPoint::from(start_position),
            LogicalPoint::from(end_position),
        )?;
        self.logical_to_projected_range_segments(logical_range)
    }

    fn verify_snapshot_version(&self, snapshot: &Snapshot) -> EngineResult<()> {
        if snapshot.version() != self.version {
            return Err(ProjectionError::VersionMismatch {
                snapshot_version: snapshot.version(),
                fold_version: self.version,
            }
            .into());
        }
        Ok(())
    }

    /// 折叠后视口切片：把 `ProjectedViewport` 翻译成投影行序列 + 命中逻辑行 spans + placeholder 列表。
    ///
    /// `snapshot` 必须与本 Projection 同版本；版本不一致返回 `ProjectionError::VersionMismatch`。
    /// `viewport.line_count` 会被自动 clamp 到投影空间总行数；超出尾部的部分被截断而不报错，
    /// 与 M11 `Snapshot::slice_viewport` 行为一致。
    pub fn slice_viewport<'a>(
        &self,
        snapshot: &'a Snapshot,
        viewport: ProjectedViewport,
    ) -> EngineResult<ProjectedViewportSlice<'a>> {
        self.verify_snapshot_version(snapshot)?;

        let total = self.line_count();
        let start = viewport.start_line().get();
        if start > total {
            return Err(crate::CoordinateError::LineOutOfBounds(Line::new(start)).into());
        }
        let end = start.saturating_add(viewport.line_count()).min(total);
        let projected_line_range =
            ProjectedLineRange::new(ProjectedLineIndex::new(start), ProjectedLineIndex::new(end));

        let mut rows: Vec<ProjectedViewportRow<'a>> = Vec::with_capacity(end - start);
        let mut placeholders: Vec<FoldPlaceholder> = Vec::new();

        for row_value in start..end {
            let index = ProjectedLineIndex::new(row_value);
            let kind = self
                .projected_line_kind(index)
                .expect("clamped row in projection range");
            match kind {
                ProjectedLineKind::Text(text_line) => {
                    let visible = build_visible_line(
                        snapshot,
                        text_line.logical_line(),
                        viewport.max_line_chars(),
                    )?;
                    rows.push(ProjectedViewportRow::new(
                        index,
                        ProjectedViewportRowKind::Text {
                            logical_line: text_line.logical_line(),
                            visible,
                        },
                    ));
                }
                ProjectedLineKind::Placeholder(placeholder) => {
                    placeholders.push(placeholder);
                    rows.push(ProjectedViewportRow::new(
                        index,
                        ProjectedViewportRowKind::Placeholder(placeholder),
                    ));
                }
            }
        }

        let logical_line_spans = build_logical_spans(&rows)?;

        Ok(ProjectedViewportSlice::new(
            viewport,
            projected_line_range,
            rows,
            logical_line_spans,
            placeholders,
        ))
    }
}

fn build_visible_line<'a>(
    snapshot: &'a Snapshot,
    logical_line: Line,
    max_line_chars: Option<usize>,
) -> EngineResult<VisibleLine<'a>> {
    let line_count = snapshot.line_count();
    let line_value = logical_line.get();
    if line_value >= line_count {
        return Err(crate::CoordinateError::LineOutOfBounds(logical_line).into());
    }

    let line_start = snapshot.line_start(logical_line)?;
    let next_start = if line_value + 1 == line_count {
        snapshot.len_chars()
    } else {
        snapshot.line_start(Line::new(line_value + 1))?
    };
    let full_range = TextRange::new(line_start, next_start)?;

    let line_slice = snapshot.slice_line(logical_line)?;
    let line_text = line_slice.as_str();
    let newline_chars = if line_text.ends_with("\r\n") {
        2
    } else if line_text.ends_with('\n') || line_text.ends_with('\r') {
        1
    } else {
        0
    };
    let content_chars = line_text.chars().count() - newline_chars;
    let content_end = CharOffset::new(line_start.get() + content_chars);

    let visible_chars = match max_line_chars {
        Some(max) => max.min(content_chars),
        None => content_chars,
    };
    let visible_end = CharOffset::new(line_start.get() + visible_chars);
    let visible_range = TextRange::new(line_start, visible_end)?;
    let is_truncated = visible_end < content_end;

    let visible_text = TextSlice::new(
        visible_range,
        snapshot.slice_text(visible_range)?.into_text(),
    );
    Ok(VisibleLine::new(
        logical_line,
        full_range,
        visible_text,
        is_truncated,
    ))
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
