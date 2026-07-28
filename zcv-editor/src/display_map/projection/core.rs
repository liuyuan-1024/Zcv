//! Editor Projection：基于 Snapshot + FoldSet 构建的不可变投影行映射快照。
//!
//! Projection 把「逻辑行」与「投影行」之间的双向映射固化成持久化摘要树：
//! - 每条投影行要么是某条可见逻辑行（`TextLine`），要么是合并后的折叠占位符（`FoldPlaceholder`）。
//! - 每条逻辑行要么可见（指向自己的投影行），要么被某段 fold 隐藏（指向 fold anchor 的投影行）。
//! - 树节点同时汇总消费的逻辑行数和产出的投影行数，双向查询由前缀摘要定位。
//!
//! 多条互相嵌套或邻接的 fold 在投影空间里只产出一条占位符（与 `HiddenRange` 合并语义保持一致）。
//! 占位符样式与像素绘制不在本类型承诺范围内。

use gpui_sum_tree::{Bias as TreeBias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use zcv_engine::{BufferVersion, CoordinateError, Line, LineRange, Snapshot, TextPatch, TextRange};

use super::{
    FoldPlaceholder, LogicalPoint, LogicalPointProjection, LogicalProjection, LogicalRange,
    ProjectedLine, ProjectedLineIndex, ProjectedLineKind, ProjectedLineRange, ProjectedPoint,
    ProjectedPointMapping, ProjectedRange, ProjectedViewport, ProjectedViewportRow,
    ProjectedViewportRowKind, ProjectedViewportSlice, TextLine, viewport::build_logical_spans,
};
use crate::display_map::{
    error::{DisplayMapResult, ProjectionError},
    fold::{FoldSet, HiddenSpan, HiddenSpanEnd},
};

/// 不可变投影行映射快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    version: BufferVersion,
    /// 投影行拓扑摘要。SumTree clone 只共享未变化节点。
    rows: SumTree<ProjectedRowItem>,
    /// 上次 build 用到的（已排序合并）隐藏行段集合，作为增量分类器的对比基准。
    /// 不暴露：只参与 `apply_patch` 的「fold 结构是否变化」判定。
    hidden_spans: SumTree<HiddenSpan>,
}

/// `Projection::apply_patch` 推进结果。语义详见对应方法文档。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// 编辑既不改变逻辑行数也不改变 fold 结构；
    /// `Projection` 仅推进 `version`，投影拓扑不改写。
    Compatible,
    /// 结构变化已定位到安全窗口，并从增量 HiddenSpan 摘要生成新的紧凑拓扑。
    Spliced,
    /// 无法安全确定局部窗口时的保守全量重建。
    Rebuilt,
}

impl Projection {
    /// 基于 `Snapshot` 与 `FoldSet` 构建一份新的投影。
    ///
    /// 要求两者版本一致；版本不匹配时返回 `ProjectionError::VersionMismatch`。
    pub fn build(snapshot: &Snapshot, folds: &FoldSet) -> DisplayMapResult<Self> {
        if snapshot.version() != folds.version() {
            return Err(ProjectionError::VersionMismatch {
                snapshot_version: snapshot.version(),
                fold_version: folds.version(),
            }
            .into());
        }

        let hidden_spans = folds.hidden_spans().clone();
        let logical_line_count = snapshot.line_count();
        let rows = build_row_items(&hidden_spans, 0, logical_line_count);

        let projection = Self {
            version: snapshot.version(),
            rows: SumTree::from_iter(rows, ()),
            hidden_spans,
        };
        debug_assert_eq!(
            projection.logical_line_count(),
            logical_line_count,
            "Projection 输入逻辑行摘要必须覆盖整个 Snapshot"
        );
        Ok(projection)
    }

    /// 尝试用跨多个连续版本组合后的 Patch 就地推进 Projection。
    ///
    /// 调用契约：
    /// - `self.version` 必须等于 `event.old_version()`；
    /// - `new_snapshot.version()` 与 `new_folds.version()` 必须等于 `event.new_version()`；
    /// - 三者任一违反契约即返回 `ProjectionError::ApplyPatchStale`，本方法不会留下半坏态。
    ///
    /// 分类策略：
    /// - **Compatible**：编辑前后 `snapshot.line_count()` 与 `hidden_spans` 都不变，
    ///   仅推进 `version`，行映射字段不改写。
    /// - **Spliced**：结构发生变化，但可从新版本 changed ranges 推导出安全的逻辑行窗口；
    ///   从增量 HiddenSpan 摘要重建紧凑拓扑，成本随 fold 段数而不是逻辑行数增长。
    /// - **Rebuilt**：fold 在变更窗口之外出现非 Delta 映射产生的拓扑变化等无法安全局部化的情况。
    ///
    /// **关键正确性性质**：分类降级到 `Rebuilt` 只是性能损失，不会产生错误投影。
    /// 故分类器允许任意保守，但绝不能把「实际不兼容」误判为 `Compatible`。
    pub fn apply_patch(
        &mut self,
        old_snapshot: &Snapshot,
        new_snapshot: &Snapshot,
        new_folds: &FoldSet,
        old_version: BufferVersion,
        new_version: BufferVersion,
        patch: &TextPatch,
    ) -> DisplayMapResult<ApplyOutcome> {
        if self.version != old_version
            || old_snapshot.version() != old_version
            || new_snapshot.version() != new_version
            || new_folds.version() != new_version
        {
            return Err(ProjectionError::ApplyPatchStale {
                projection_version: self.version,
                patch_old_version: old_version,
                patch_new_version: new_version,
                snapshot_version: new_snapshot.version(),
                fold_version: new_folds.version(),
            }
            .into());
        }

        let old_line_count = self.logical_line_count();
        let line_count_unchanged = new_snapshot.line_count() == old_line_count;
        let text_topology_unchanged = line_count_unchanged
            && patch.edits().iter().all(|edit| {
                old_snapshot
                    .slice_text(edit.old_range())
                    .is_ok_and(|text| !text.as_str().contains('\n'))
                    && new_snapshot
                        .slice_text(edit.new_range())
                        .is_ok_and(|text| !text.as_str().contains('\n'))
            });
        let new_spans = new_folds.hidden_spans().clone();
        let hidden_topology_unchanged = new_folds
            .hidden_spans_changed_between(old_version, new_version)
            .map_or_else(|| new_spans == self.hidden_spans, |changed| !changed);
        if text_topology_unchanged && hidden_topology_unchanged {
            self.version = new_version;
            return Ok(ApplyOutcome::Compatible);
        }

        if !new_folds.was_updated_between(old_version, new_version) {
            let rebuilt = Self::build(new_snapshot, new_folds)?;
            *self = rebuilt;
            return Ok(ApplyOutcome::Rebuilt);
        }

        if let Some(window) = self.splice_window(new_snapshot, &new_spans, patch)? {
            let _ = window;
            self.rows = SumTree::from_iter(
                build_row_items(&new_spans, 0, new_snapshot.line_count()),
                (),
            );
            self.hidden_spans = new_spans;
            self.version = new_version;
            debug_assert_eq!(self.logical_line_count(), new_snapshot.line_count());
            return Ok(ApplyOutcome::Spliced);
        }

        // 保守降级：无法证明窗口外拓扑可复用时全量重建。
        let rebuilt = Self::build(new_snapshot, new_folds)?;
        *self = rebuilt;
        Ok(ApplyOutcome::Rebuilt)
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    /// 投影行总数；等价于 `text line 数 + fold placeholder 数`。
    pub fn line_count(&self) -> usize {
        self.rows.summary().rows
    }

    /// 来源文本的逻辑行总数。
    pub fn logical_line_count(&self) -> usize {
        self.rows.summary().logical_lines
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn summary_item_count(&self) -> usize {
        self.rows.iter().count()
    }

    /// 按投影行索引读取行视图；越界返回 None。
    pub fn projected_line(&self, index: ProjectedLineIndex) -> Option<ProjectedLine> {
        self.projected_line_kind(index)
            .map(|kind| ProjectedLine::new(index, kind))
    }

    /// 顺序遍历所有投影行（带索引）。
    pub fn iter(&self) -> impl Iterator<Item = ProjectedLine> + '_ {
        self.rows
            .iter()
            .scan((0usize, 0usize), |state, row| {
                let (logical_start, projected_start) = *state;
                let lines = (0..row.projected_rows())
                    .map(|offset| {
                        ProjectedLine::new(
                            ProjectedLineIndex::new(projected_start + offset),
                            row.projected_kind(logical_start, offset),
                        )
                    })
                    .collect::<Vec<_>>();
                state.0 += row.logical_lines;
                state.1 += row.projected_rows();
                Some(lines.into_iter())
            })
            .flatten()
    }

    /// 投影行索引 -> 投影行种类（不带索引包裹）。
    pub fn projected_line_kind(&self, index: ProjectedLineIndex) -> Option<ProjectedLineKind> {
        let (start, _, row) = self.rows.find::<ProjectedDimensions, _>(
            (),
            &ProjectedRowCount(index.get()),
            TreeBias::Right,
        );
        row.map(|row| row.projected_kind(start.1.0, index.get() - start.0.0))
    }

    /// 逻辑行 -> 投影空间状态。
    pub fn logical_to_projected(&self, line: Line) -> DisplayMapResult<LogicalProjection> {
        if line.get() >= self.logical_line_count() {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }
        let (start, _, row) = self.rows.find::<ProjectionDimensions, _>(
            (),
            &LogicalLineCount(line.get()),
            TreeBias::Right,
        );
        let row = row.expect("合法逻辑行必须命中 Projection 变换项");
        Ok(match row.kind {
            ProjectedRowItemKind::Text => LogicalProjection::Visible(ProjectedLineIndex::new(
                start.1.0 + line.get() - start.0.0,
            )),
            ProjectedRowItemKind::Placeholder => LogicalProjection::Hidden {
                anchor_logical_line: Line::new(start.0.0 - 1),
                anchor_projected_line: ProjectedLineIndex::new(start.1.0 - 1),
            },
        })
    }

    pub fn is_logical_line_hidden(&self, line: Line) -> DisplayMapResult<bool> {
        Ok(self.logical_to_projected(line)?.is_hidden())
    }

    /// hidden 逻辑行 -> 其所在 fold 的 anchor 逻辑行。可见行返回自身。
    pub fn fold_anchor_for_logical_line(&self, line: Line) -> DisplayMapResult<Line> {
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
    ) -> DisplayMapResult<LogicalPointProjection> {
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
    ) -> DisplayMapResult<ProjectedPointMapping> {
        let kind = self
            .projected_line_kind(point.line)
            .ok_or_else(|| CoordinateError::LineOutOfBounds(Line::new(point.line.get())))?;

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
    ) -> DisplayMapResult<Vec<ProjectedRange>> {
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
            .is_some_and(|kind| kind.is_placeholder());

        for row_value in (start_proj.line().get() + 1)..=end_proj.line().get() {
            let row_idx = ProjectedLineIndex::new(row_value);
            let row_is_placeholder = self
                .projected_line_kind(row_idx)
                .is_some_and(|kind| kind.is_placeholder());
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
    pub fn projected_to_logical_range(
        &self,
        range: ProjectedRange,
    ) -> DisplayMapResult<LogicalRange> {
        let start_logical = self.projected_point_to_logical_range_endpoint(range.start(), true)?;
        let end_logical = self.projected_point_to_logical_range_endpoint(range.end(), false)?;
        Ok(LogicalRange::new(start_logical, end_logical)?)
    }

    fn projected_point_to_logical_range_endpoint(
        &self,
        point: ProjectedPoint,
        is_start: bool,
    ) -> DisplayMapResult<LogicalPoint> {
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
        range: TextRange,
    ) -> DisplayMapResult<Vec<ProjectedRange>> {
        self.verify_snapshot_version(snapshot)?;
        let start_position = snapshot.byte_to_position(range.start())?;
        let end_position = snapshot.byte_to_position(range.end())?;
        let logical_range = LogicalRange::new(
            LogicalPoint::from(start_position),
            LogicalPoint::from(end_position),
        )?;
        self.logical_to_projected_range_segments(logical_range)
    }

    fn verify_snapshot_version(&self, snapshot: &Snapshot) -> DisplayMapResult<()> {
        if snapshot.version() != self.version {
            return Err(ProjectionError::VersionMismatch {
                snapshot_version: snapshot.version(),
                fold_version: self.version,
            }
            .into());
        }
        Ok(())
    }

    fn splice_window(
        &self,
        new_snapshot: &Snapshot,
        new_spans: &SumTree<HiddenSpan>,
        patch: &TextPatch,
    ) -> DisplayMapResult<Option<SpliceWindow>> {
        let Some(first) = patch.edits().first() else {
            return Ok(None);
        };
        let last = patch.edits().last().expect("非空 Patch 必须存在末项");

        let old_line_count = self.logical_line_count();
        let new_line_count = new_snapshot.line_count();
        let Some(line_delta) = signed_difference(new_line_count, old_line_count) else {
            return Ok(None);
        };

        // 第一个 edit 之前没有字节变化，因此首个受影响逻辑行在新旧版本中相同。
        let mut new_start = new_snapshot.byte_to_line(first.new_range().start())?.get();
        let mut old_start = new_start;

        // 取最后 changed range 所在行的下一行作为稳定后缀边界。
        let changed_end_line = new_snapshot.byte_to_line(last.new_range().end())?.get();
        let mut new_end = changed_end_line.saturating_add(1).min(new_line_count);
        let Some(mut old_end) = new_end.checked_add_signed(-line_delta) else {
            return Ok(None);
        };
        if old_end > old_line_count {
            return Ok(None);
        }

        // splice 不能切开 placeholder。两端在新旧隐藏段中反复扩张到完整边界。
        loop {
            let previous = (old_start, new_start);
            if let Some(span) = containing_span(&self.hidden_spans, old_start) {
                old_start = span.start.get();
                new_start = old_start;
            }
            if let Some(span) = containing_span(new_spans, new_start) {
                new_start = span.start.get();
                old_start = new_start;
            }
            if previous == (old_start, new_start) {
                break;
            }
        }

        loop {
            let previous = (old_end, new_end);
            if let Some(span) = containing_span(&self.hidden_spans, old_end) {
                old_end = span.end.get();
                let Some(mapped) = old_end.checked_add_signed(line_delta) else {
                    return Ok(None);
                };
                new_end = mapped;
            }
            if let Some(span) = containing_span(new_spans, new_end) {
                new_end = span.end.get();
                let Some(mapped) = new_end.checked_add_signed(-line_delta) else {
                    return Ok(None);
                };
                old_end = mapped;
            }
            if old_end > old_line_count || new_end > new_line_count {
                return Ok(None);
            }
            if previous == (old_end, new_end) {
                break;
            }
        }

        if old_start > old_end || new_start > new_end {
            return Ok(None);
        }
        Ok(Some(SpliceWindow {
            old_start,
            old_end,
            new_start,
            new_end,
        }))
    }

    /// 折叠后视口切片：把 `ProjectedViewport` 翻译成投影行序列 + 命中逻辑行 spans + placeholder 列表。
    ///
    /// `snapshot` 必须与本 Projection 同版本；版本不一致返回 `ProjectionError::VersionMismatch`。
    /// `viewport.line_count` 会被自动 clamp 到投影空间总行数；超出尾部的部分被截断而不报错，
    /// 与 `Snapshot::slice_viewport` 行为一致。
    pub fn slice_viewport<'a>(
        &self,
        snapshot: &'a Snapshot,
        viewport: ProjectedViewport,
    ) -> DisplayMapResult<ProjectedViewportSlice<'a>> {
        self.verify_snapshot_version(snapshot)?;

        let total = self.line_count();
        let start = viewport.start_line().get();
        if start > total {
            return Err(CoordinateError::LineOutOfBounds(Line::new(start)).into());
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
                .expect("内部不变量：clamp 后的 row 必须位于 projection 范围内");
            match kind {
                ProjectedLineKind::Text(text_line) => {
                    let visible = snapshot
                        .visible_line(text_line.logical_line(), viewport.max_line_chars())?;
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectedRowItem {
    kind: ProjectedRowItemKind,
    logical_lines: usize,
}

impl ProjectedRowItem {
    fn text_run(line_count: usize) -> Self {
        debug_assert!(line_count > 0);
        Self {
            kind: ProjectedRowItemKind::Text,
            logical_lines: line_count,
        }
    }

    fn placeholder(hidden_line_count: usize) -> Self {
        debug_assert!(hidden_line_count > 0);
        Self {
            kind: ProjectedRowItemKind::Placeholder,
            logical_lines: hidden_line_count,
        }
    }

    fn projected_rows(&self) -> usize {
        match self.kind {
            ProjectedRowItemKind::Text => self.logical_lines,
            ProjectedRowItemKind::Placeholder => 1,
        }
    }

    fn projected_kind(&self, logical_start: usize, offset: usize) -> ProjectedLineKind {
        match self.kind {
            ProjectedRowItemKind::Text => {
                debug_assert!(offset < self.logical_lines);
                ProjectedLineKind::Text(TextLine::new(Line::new(logical_start + offset)))
            }
            ProjectedRowItemKind::Placeholder => {
                debug_assert_eq!(offset, 0);
                let hidden_start = Line::new(logical_start);
                let hidden_end = Line::new(logical_start + self.logical_lines);
                ProjectedLineKind::Placeholder(FoldPlaceholder::new(
                    Line::new(logical_start - 1),
                    LineRange::new(hidden_start, hidden_end)
                        .expect("placeholder 必须覆盖非空且有序的隐藏行区间"),
                ))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectedRowItemKind {
    Text,
    Placeholder,
}

impl Item for ProjectedRowItem {
    type Summary = ProjectedRowSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        ProjectedRowSummary {
            rows: self.projected_rows(),
            logical_lines: self.logical_lines,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProjectedRowSummary {
    rows: usize,
    logical_lines: usize,
}

impl ContextLessSummary for ProjectedRowSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.rows += summary.rows;
        self.logical_lines += summary.logical_lines;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct LogicalLineCount(usize);

impl<'a> Dimension<'a, ProjectedRowSummary> for LogicalLineCount {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a ProjectedRowSummary, (): ()) {
        self.0 += summary.logical_lines;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct ProjectedRowCount(usize);

impl<'a> Dimension<'a, ProjectedRowSummary> for ProjectedRowCount {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a ProjectedRowSummary, (): ()) {
        self.0 += summary.rows;
    }
}

type ProjectionDimensions = Dimensions<LogicalLineCount, ProjectedRowCount>;
type ProjectedDimensions = Dimensions<ProjectedRowCount, LogicalLineCount>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SpliceWindow {
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
}

fn build_row_items(
    hidden_spans: &SumTree<HiddenSpan>,
    start: usize,
    end: usize,
) -> Vec<ProjectedRowItem> {
    debug_assert!(start <= end);

    let mut rows = Vec::new();
    let mut spans = hidden_spans.cursor::<HiddenSpanEnd>(());
    spans.seek(&HiddenSpanEnd(start), TreeBias::Right);
    let mut line_value = start;
    while let Some(span) = spans.item().copied() {
        if span.start.get() >= end {
            break;
        }
        if span.start.get() > line_value {
            rows.push(ProjectedRowItem::text_run(span.start.get() - line_value));
        }
        debug_assert!(span.end.get() <= end);
        rows.push(ProjectedRowItem::placeholder(
            span.end.get() - span.start.get(),
        ));
        line_value = span.end.get();
        spans.next();
    }
    if line_value < end {
        rows.push(ProjectedRowItem::text_run(end - line_value));
    }
    rows
}

fn containing_span(spans: &SumTree<HiddenSpan>, boundary: usize) -> Option<HiddenSpan> {
    let (_, _, span) =
        spans.find::<HiddenSpanEnd, _>((), &HiddenSpanEnd(boundary), TreeBias::Right);
    span.copied()
        .filter(|span| span.start.get() < boundary && boundary < span.end.get())
}

fn signed_difference(left: usize, right: usize) -> Option<isize> {
    if left >= right {
        isize::try_from(left - right).ok()
    } else {
        isize::try_from(right - left).ok().map(|value| -value)
    }
}
