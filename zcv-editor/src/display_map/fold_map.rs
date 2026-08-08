//! 折叠显示层。
//!
//! FoldMap 是唯一写入口，向上层发布 FoldSnapshot 与 FoldEdit。

use std::{cmp::Reverse, collections::BTreeMap, ops::Range};

use gpui_sum_tree::{Bias as TreeBias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use zcv_engine::{
    BufferVersion, ByteOffset, CoordinateError, Line, LineRange, LogicalColumn, Position,
    PositionMap, Snapshot, Stickiness, TextChangeBatch, TextRange, TrackedRange,
    TrackedRangeUpdatePolicy,
};

use super::error::{DisplayMapResult, FoldError};
use super::inlay_map::InlaySnapshot;
use super::line_stream::{LineStream, StreamLineSource};
use super::tab_map::line_content;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyOutcome {
    Compatible,
    Spliced,
    Rebuilt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct FoldId(u64);

impl FoldId {
    const INITIAL: Self = Self(1);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fold {
    id: FoldId,
    range: TrackedRange,
    update_policy: TrackedRangeUpdatePolicy,
    line_span: (Line, Line),
}

impl Fold {
    fn new(
        id: FoldId,
        version: BufferVersion,
        range: TextRange,
        update_policy: TrackedRangeUpdatePolicy,
        line_span: (Line, Line),
    ) -> Self {
        Self {
            id,
            range: TrackedRange::from_range(version, range, Stickiness::Never),
            update_policy,
            line_span,
        }
    }

    fn text_range(self) -> TextRange {
        self.range.range()
    }
}

impl Item for Fold {
    type Summary = FoldSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        FoldSummary {
            count: 1,
            last_order: FoldOrder::for_fold(*self),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FoldSummary {
    count: usize,
    last_order: FoldOrder,
}

impl Default for FoldSummary {
    fn default() -> Self {
        Self {
            count: 0,
            last_order: FoldOrder::zero(()),
        }
    }
}

impl ContextLessSummary for FoldSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.count += summary.count;
        self.last_order = summary.last_order;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FoldOrder {
    start: usize,
    end_descending: Reverse<usize>,
    id: FoldId,
}

impl FoldOrder {
    fn for_fold(fold: Fold) -> Self {
        Self {
            start: fold.text_range().start().get(),
            end_descending: Reverse(fold.text_range().end().get()),
            id: fold.id,
        }
    }
}

impl<'a> Dimension<'a, FoldSummary> for FoldOrder {
    fn zero((): ()) -> Self {
        Self {
            start: 0,
            end_descending: Reverse(usize::MAX),
            id: FoldId(0),
        }
    }

    fn add_summary(&mut self, summary: &'a FoldSummary, (): ()) {
        *self = summary.last_order;
    }
}

/// 隐藏点投影的 bias 约定（对齐 Zed `to_fold_point`）：Left 吸附折叠起点列，Right 吸附折叠终点列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FoldBias {
    Left,
    Right,
}

/// 折叠占位符文本：折叠后拼在 anchor 行文本之后，与闭合行尾段处于同一显示行。
pub(crate) const FOLD_PLACEHOLDER: &str = "…";

/// 占位符字符数（列换算用，占一列）。
const FOLD_PLACEHOLDER_CHARS: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransformKind {
    Isomorphic,
    Fold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Transform {
    kind: TransformKind,
    input_lines: usize,
}

impl Transform {
    fn isomorphic(lines: usize) -> Self {
        Self {
            kind: TransformKind::Isomorphic,
            input_lines: lines,
        }
    }

    fn fold(lines: usize) -> Self {
        Self {
            kind: TransformKind::Fold,
            input_lines: lines,
        }
    }

    fn output_rows(self) -> usize {
        match self.kind {
            TransformKind::Isomorphic => self.input_lines,
            // 折叠段完全吞掉输入行，不产生投影行；折叠省略号绘制在 anchor 行行尾。
            TransformKind::Fold => 0,
        }
    }

    fn projected_kind(self, logical_start: usize, offset: usize) -> TextLine {
        match self.kind {
            TransformKind::Isomorphic => TextLine::new(Line::new(logical_start + offset)),
            TransformKind::Fold => unreachable!("fold 段不产生投影行"),
        }
    }
}

impl Item for Transform {
    type Summary = TransformSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        TransformSummary {
            input_lines: self.input_lines,
            output_rows: self.output_rows(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TransformSummary {
    input_lines: usize,
    output_rows: usize,
}

impl ContextLessSummary for TransformSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.input_lines += summary.input_lines;
        self.output_rows += summary.output_rows;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct InputLines(usize);

impl<'a> Dimension<'a, TransformSummary> for InputLines {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.0 += summary.input_lines;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct OutputRows(usize);

impl<'a> Dimension<'a, TransformSummary> for OutputRows {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.0 += summary.output_rows;
    }
}

type InputToOutput = Dimensions<InputLines, OutputRows>;
type OutputToInput = Dimensions<OutputRows, InputLines>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FoldEdit {
    old: Range<ProjectedLineIndex>,
    new: Range<ProjectedLineIndex>,
    changed_lines: Vec<Line>,
    structural: bool,
}

impl FoldEdit {
    pub(super) fn changed_lines(&self) -> &[Line] {
        &self.changed_lines
    }

    pub(super) fn is_structural(&self) -> bool {
        self.structural || self.old.start != self.new.start || self.old.end != self.new.end
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FoldSnapshot {
    /// 输入投影快照（行内提示注入后的流）：fold 拓扑工作在其上，外部文本可被折叠。
    input: InlaySnapshot,
    folds: SumTree<Fold>,
    transforms: SumTree<Transform>,
    fold_metadata_by_id: BTreeMap<FoldId, TextRange>,
    version: u64,
}

impl FoldSnapshot {
    pub(super) fn stream(&self) -> &LineStream {
        self.input.stream()
    }

    pub(crate) fn inlay_snapshot(&self) -> &InlaySnapshot {
        &self.input
    }

    pub(super) fn buffer_snapshot(&self) -> &Snapshot {
        self.input.buffer_snapshot()
    }

    pub(super) const fn version(&self) -> u64 {
        self.version
    }

    pub(super) fn line_count(&self) -> usize {
        self.transforms.summary().output_rows
    }

    /// 折叠入口行（anchor 行行尾绘制折叠省略号；无隐藏行的 fold 不计）。
    pub(crate) fn fold_anchor_lines(&self) -> Vec<Line> {
        self.folds
            .iter()
            .filter_map(|fold| {
                let (start, end) = fold.line_span;
                (start < end).then_some(start)
            })
            .collect()
    }

    /// 折叠起点括号对应的另一半闭合符（`{`→`}`、`(`→`)`、`[`→`]`），anchor 行行尾省略号后绘制成 `{...}` 样式。
    ///
    /// 从折叠范围起点行的最后一个非空白字符推导配对；注释组等无括号折叠返回 None。
    pub(crate) fn fold_end_char(&self, anchor: Line, snapshot: &Snapshot) -> Option<char> {
        let fold = self.folds.iter().find(|fold| {
            let (start, end) = fold.line_span;
            start == anchor && start < end
        })?;
        let line_text = snapshot.slice_line(fold.line_span.0).ok()?;
        let open = line_text
            .as_str()
            .trim_end_matches(['\r', '\n'])
            .chars()
            .rev()
            .find(|c| !c.is_whitespace())?;
        match open {
            '{' => Some('}'),
            '(' => Some(')'),
            '[' => Some(']'),
            _ => None,
        }
    }

    fn logical_line_count(&self) -> usize {
        // fold 拓扑的输入行 = 流行（buffer + 合成）。
        self.transforms.summary().input_lines
    }

    pub(crate) fn projected_line_kind(&self, index: ProjectedLineIndex) -> Option<TextLine> {
        let (start, _, transform) =
            self.transforms
                .find::<OutputToInput, _>((), &OutputRows(index.get()), TreeBias::Right);
        transform.map(|transform| {
            transform.projected_kind(start.1.0, index.get().saturating_sub(start.0.0))
        })
    }

    pub(super) fn logical_to_projected(&self, line: Line) -> DisplayMapResult<LogicalProjection> {
        if line.get() >= self.logical_line_count() {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }
        let (start, _, transform) =
            self.transforms
                .find::<InputToOutput, _>((), &InputLines(line.get()), TreeBias::Right);
        let transform = transform.expect("逻辑行必须落在 fold transform 内");
        Ok(match transform.kind {
            TransformKind::Isomorphic => LogicalProjection::Visible(ProjectedLineIndex::new(
                start.1.0 + line.get() - start.0.0,
            )),
            TransformKind::Fold => LogicalProjection::Hidden {
                anchor_logical_line: Line::new(start.0.0 - 1),
                anchor_projected_line: ProjectedLineIndex::new(start.1.0 - 1),
            },
        })
    }

    /// 逻辑点 → 投影点（列在合并行文本空间中）。
    ///
    /// 折叠覆盖行（anchor 与 close 之间的隐藏行、close 行隐藏前缀）按 bias 吸附到折叠起点/终点列；
    /// close 行可见尾段投影到占位符之后。
    pub(super) fn logical_to_projected_point(
        &self,
        point: LogicalPoint,
        bias: FoldBias,
    ) -> DisplayMapResult<ProjectedPoint> {
        if let Some(fold) = self.fold_covering(point.line()) {
            let geometry = self.fold_merged_geometry(fold)?;
            let close_line = fold.line_span.1;
            if point.line() == close_line && point.column().get() >= geometry.tail_start_col {
                let column = geometry.anchor_chars
                    + FOLD_PLACEHOLDER_CHARS
                    + self.tail_projected_column(&geometry, point.column().get())?;
                return Ok(ProjectedPoint::new(
                    geometry.row,
                    LogicalColumn::new(column),
                ));
            }
            let column = match bias {
                FoldBias::Left => geometry.anchor_chars,
                FoldBias::Right => geometry.anchor_chars + FOLD_PLACEHOLDER_CHARS,
            };
            return Ok(ProjectedPoint::new(
                geometry.row,
                LogicalColumn::new(column),
            ));
        }
        match self.logical_to_projected(point.line())? {
            LogicalProjection::Visible(line) => Ok(ProjectedPoint::new(line, point.column())),
            LogicalProjection::Hidden { .. } => unreachable!("fold_covering 已覆盖全部隐藏行"),
        }
    }

    /// 覆盖该逻辑行的最外层折叠（anchor 与 close 之间的隐藏行，含 close 行）。
    fn fold_covering(&self, line: Line) -> Option<&Fold> {
        self.folds
            .iter()
            .filter(|fold| {
                let (start, end) = fold.line_span;
                start < end && start < line && line <= end
            })
            .min_by_key(|fold| fold.line_span.0)
    }

    /// 折叠合并行的投影几何（anchor 行文本 + 占位符 + 闭合行尾段）。
    fn fold_merged_geometry(&self, fold: &Fold) -> DisplayMapResult<FoldMergedGeometry> {
        let anchor = fold.line_span.0;
        let row = match self.logical_to_projected(anchor)? {
            LogicalProjection::Visible(row) => row,
            LogicalProjection::Hidden { .. } => unreachable!("折叠 anchor 行必须可见"),
        };
        let inlay = self.inlay_snapshot();
        let stream = self.stream();
        let anchor_stream = stream.buffer_to_stream(anchor);
        let anchor_text = inlay
            .line_text(anchor_stream)
            .expect("折叠 anchor 行必须位于流内");
        let anchor_chars = line_content(anchor_text.as_ref()).chars().count();
        let close_line = fold.line_span.1;
        let close_stream = stream.buffer_to_stream(close_line);
        let close_start = self
            .buffer_snapshot()
            .line_start_byte(close_line)
            .expect("折叠 close 行必须位于当前 Snapshot 内");
        let tail_byte = fold.text_range().end().get() - close_start.get();
        let tail_projected = inlay.to_projected_offset(close_stream, tail_byte);
        let tail_start_col = self
            .buffer_snapshot()
            .byte_to_position(fold.text_range().end())
            .ok()
            .map_or(0, |position| position.column().get());
        Ok(FoldMergedGeometry {
            row,
            anchor_chars,
            close_line,
            tail_byte,
            tail_projected,
            tail_start_col,
        })
    }

    /// close 行尾段内逻辑列 → 合并行内的投影列（尾段起点之后，含注入）。
    fn tail_projected_column(
        &self,
        geometry: &FoldMergedGeometry,
        column: usize,
    ) -> DisplayMapResult<usize> {
        let buffer = self.buffer_snapshot();
        let close_start = buffer.line_start_byte(geometry.close_line)?;
        let byte = buffer
            .position_to_byte(Position::new(
                geometry.close_line,
                LogicalColumn::new(column),
            ))?
            .get();
        let projected = self.inlay_snapshot().to_projected_offset(
            self.stream().buffer_to_stream(geometry.close_line),
            byte - close_start.get(),
        );
        Ok(projected - geometry.tail_projected)
    }

    /// 投影行的内容来源：fold 投影（Text）叠加流行解析。
    ///
    /// 文本行统一携带流来源（buffer 行 / 合成行），fold/tab/wrap 层不感知"合成行"概念；
    /// 删除块展开的外部文本就是普通行，渲染端按来源区分行号与可命中性。
    pub(super) fn projected_kind(
        &self,
        projected: ProjectedLineIndex,
    ) -> Option<StreamProjectedKind> {
        let text = self.projected_line_kind(projected)?;
        Some(StreamProjectedKind::Text(
            self.input
                .source(text.logical_line())
                .expect("可见投影行必须落在流内"),
        ))
    }
}

/// 投影行（fold 输出）的内容来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamProjectedKind {
    /// 可见文本行（buffer 行或合成行，经流解析）。
    Text(StreamLineSource),
}

#[derive(Debug, Clone)]
pub(super) struct FoldMap {
    snapshot: FoldSnapshot,
    next_fold_id: FoldId,
    default_update_policy: TrackedRangeUpdatePolicy,
}

impl FoldMap {
    pub(super) fn new(input: InlaySnapshot) -> (Self, FoldSnapshot) {
        let transforms = build_transforms(&[], input.line_count());
        let snapshot = FoldSnapshot {
            input,
            folds: SumTree::new(()),
            transforms,
            fold_metadata_by_id: BTreeMap::new(),
            version: 0,
        };
        (
            Self {
                snapshot: snapshot.clone(),
                next_fold_id: FoldId::INITIAL,
                default_update_policy: TrackedRangeUpdatePolicy::invalidate_when_fully_deleted(),
            },
            snapshot,
        )
    }

    pub(super) fn snapshot(&self) -> &FoldSnapshot {
        &self.snapshot
    }

    pub(super) fn read(
        &mut self,
        input: InlaySnapshot,
        batch: &TextChangeBatch,
    ) -> (FoldSnapshot, Vec<FoldEdit>, ApplyOutcome) {
        let buffer = input.buffer_snapshot();
        let old_buffer = self.snapshot.buffer_snapshot().clone();
        let inserted_changed =
            input.stream().inserted_version() != self.snapshot.stream().inserted_version();
        // 注入配置变化（inlay 增删改）不产生 buffer 编辑：整体重建 fold 拓扑。
        let inlay_changed = input.version() != self.snapshot.input.version();
        if buffer.version() == old_buffer.version() && !inserted_changed && !inlay_changed {
            if buffer.config() != old_buffer.config() {
                let old_end = ProjectedLineIndex::new(self.snapshot.line_count());
                self.snapshot.input = input;
                self.snapshot.version += 1;
                let edit = FoldEdit {
                    old: ProjectedLineIndex::ZERO..old_end,
                    new: ProjectedLineIndex::ZERO
                        ..ProjectedLineIndex::new(self.snapshot.line_count()),
                    changed_lines: Vec::new(),
                    structural: true,
                };
                return (self.snapshot.clone(), vec![edit], ApplyOutcome::Compatible);
            }
            return (self.snapshot.clone(), Vec::new(), ApplyOutcome::Compatible);
        }

        let old_version = old_buffer.version();
        let new_version = buffer.version();
        // 合成行/行内提示配置变化（删除块展开/折叠、inlay 注入）不产生 buffer 编辑：整体重建 fold 拓扑。
        if inserted_changed || inlay_changed {
            let old_rows = self.snapshot.line_count();
            self.snapshot = FoldSnapshot {
                transforms: build_transforms(&[], input.line_count()),
                input,
                folds: self.snapshot.folds.clone(),
                fold_metadata_by_id: self.snapshot.fold_metadata_by_id.clone(),
                version: self.snapshot.version + 1,
            };
            let edit = full_fold_edit(old_rows, self.snapshot.line_count());
            return (self.snapshot.clone(), vec![edit], ApplyOutcome::Rebuilt);
        }
        if batch.requires_reset()
            || batch.old_version() != Some(old_version)
            || batch.new_version() != Some(new_version)
        {
            let old_rows = self.snapshot.line_count();
            self.snapshot = FoldSnapshot {
                transforms: build_transforms(&[], input.line_count()),
                input,
                folds: SumTree::new(()),
                fold_metadata_by_id: BTreeMap::new(),
                version: self.snapshot.version + 1,
            };
            let edit = full_fold_edit(old_rows, self.snapshot.line_count());
            return (self.snapshot.clone(), vec![edit], ApplyOutcome::Rebuilt);
        }

        let old_rows = self.snapshot.line_count();
        let old_spans = hidden_spans(&self.snapshot.folds);
        let position_map = PositionMap::from_text_patch(batch.patch());
        let mut retained = Vec::new();
        self.snapshot.fold_metadata_by_id.clear();
        for mut fold in self.snapshot.folds.iter().copied() {
            let update = fold.range.map_through_position_map_with_policy(
                new_version,
                &position_map,
                fold.update_policy,
            );
            if let Some(range) = update.tracked_range() {
                fold.range = range;
                fold.line_span = fold_line_span(buffer, range.range())
                    .expect("映射后的 tracked fold 必须位于当前 Snapshot 内");
                self.snapshot
                    .fold_metadata_by_id
                    .insert(fold.id, range.range());
                retained.push(fold);
            }
        }
        sort_folds(&mut retained);
        self.snapshot.folds = SumTree::from_iter(retained, ());
        let new_spans = hidden_spans(&self.snapshot.folds);
        // 行数不变且折叠拓扑不变时，编辑只改变与 new_range 相交行的内容，
        // `inline_fold_edits` 的 changed_lines 恰好覆盖；软换行的逐行重排（update_inline）
        // 对行数不变的多行编辑同样正确，无需按"含换行"升级为全量重建。
        let structural = old_spans != new_spans || old_buffer.line_count() != buffer.line_count();
        self.snapshot.input = input;
        self.snapshot.version += 1;
        let outcome = if structural {
            let stream = self.snapshot.stream();
            let spans = hidden_spans_in_stream(stream, &self.snapshot.folds);
            self.snapshot.transforms = build_transforms(&spans, stream.line_count());
            ApplyOutcome::Spliced
        } else {
            ApplyOutcome::Compatible
        };
        let edits = if structural {
            vec![full_fold_edit(old_rows, self.snapshot.line_count())]
        } else {
            inline_fold_edits(batch, self.snapshot.stream(), &self.snapshot.folds)
        };
        (self.snapshot.clone(), edits, outcome)
    }

    pub(super) fn write(&mut self) -> FoldMapWriter<'_> {
        FoldMapWriter(self)
    }
}

pub(super) struct FoldMapWriter<'a>(&'a mut FoldMap);

impl FoldMapWriter<'_> {
    pub(super) fn fold_lines(
        &mut self,
        line_range: LineRange,
    ) -> DisplayMapResult<(FoldSnapshot, Vec<FoldEdit>)> {
        let range = text_range_for_folded_lines(self.0.snapshot.buffer_snapshot(), line_range)?;
        self.fold(range)
    }

    /// 展开与行范围交叠的全部折叠（半开区间，约定同 `fold_lines`）。
    pub(super) fn unfold_lines(
        &mut self,
        line_range: LineRange,
    ) -> DisplayMapResult<(FoldSnapshot, Vec<FoldEdit>)> {
        let ids: Vec<_> = self
            .0
            .snapshot
            .folds
            .iter()
            .filter(|fold| {
                let (start, end) = fold.line_span;
                start.get() < line_range.end().get() && end.get() >= line_range.start().get()
            })
            .map(|fold| fold.id)
            .collect();
        if ids.is_empty() {
            return Ok((self.0.snapshot.clone(), Vec::new()));
        }
        let mut snapshot = self.0.snapshot.clone();
        let mut edits = Vec::new();
        for id in ids {
            let (next, edit) = self.unfold(id);
            snapshot = next;
            edits.extend(edit);
        }
        Ok((snapshot, edits))
    }

    fn fold(&mut self, range: TextRange) -> DisplayMapResult<(FoldSnapshot, Vec<FoldEdit>)> {
        if range.is_empty() {
            return Err(FoldError::EmptyRange { range }.into());
        }
        if self
            .0
            .snapshot
            .folds
            .iter()
            .any(|fold| fold.text_range() == range)
        {
            return Ok((self.0.snapshot.clone(), Vec::new()));
        }
        for fold in self.0.snapshot.folds.iter() {
            if !ranges_disjoint_or_nested(fold.text_range(), range) {
                return Err(FoldError::OverlapWithoutNesting {
                    existing: fold.text_range(),
                    candidate: range,
                }
                .into());
            }
        }
        let id = self.0.next_fold_id;
        self.0.next_fold_id = FoldId(
            self.0
                .next_fold_id
                .0
                .checked_add(1)
                .ok_or(FoldError::IdOverflow)?,
        );
        let mut folds: Vec<_> = self.0.snapshot.folds.iter().copied().collect();
        let fold = Fold::new(
            id,
            self.0.snapshot.buffer_snapshot().version(),
            range,
            self.0.default_update_policy,
            fold_line_span(self.0.snapshot.buffer_snapshot(), range)?,
        );
        folds.push(fold);
        sort_folds(&mut folds);
        let old_rows = self.0.snapshot.line_count();
        self.0.snapshot.folds = SumTree::from_iter(folds, ());
        self.0.snapshot.fold_metadata_by_id.insert(id, range);
        let spans = hidden_spans_in_stream(self.0.snapshot.stream(), &self.0.snapshot.folds);
        self.0.snapshot.transforms =
            build_transforms(&spans, self.0.snapshot.stream().line_count());
        self.0.snapshot.version += 1;
        let edit = full_fold_edit(old_rows, self.0.snapshot.line_count());
        Ok((self.0.snapshot.clone(), vec![edit]))
    }

    fn unfold(&mut self, id: FoldId) -> (FoldSnapshot, Vec<FoldEdit>) {
        if !self.0.snapshot.fold_metadata_by_id.contains_key(&id) {
            return (self.0.snapshot.clone(), Vec::new());
        }
        let old_rows = self.0.snapshot.line_count();
        let retained: Vec<_> = self
            .0
            .snapshot
            .folds
            .iter()
            .copied()
            .filter(|fold| fold.id != id)
            .collect();
        self.0.snapshot.folds = SumTree::from_iter(retained, ());
        self.0.snapshot.fold_metadata_by_id.remove(&id);
        let spans = hidden_spans_in_stream(self.0.snapshot.stream(), &self.0.snapshot.folds);
        self.0.snapshot.transforms =
            build_transforms(&spans, self.0.snapshot.stream().line_count());
        self.0.snapshot.version += 1;
        let edit = full_fold_edit(old_rows, self.0.snapshot.line_count());
        (self.0.snapshot.clone(), vec![edit])
    }
}

fn sort_folds(folds: &mut [Fold]) {
    folds.sort_by_key(|fold| FoldOrder::for_fold(*fold));
}

fn hidden_spans(folds: &SumTree<Fold>) -> Vec<Range<usize>> {
    let mut spans: Vec<Range<usize>> = Vec::new();
    for fold in folds.iter() {
        let (start, end) = fold.line_span;
        if start >= end {
            continue;
        }
        let span = start.get() + 1..end.get() + 1;
        match spans.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => spans.push(span),
        }
    }
    spans
}

/// 折叠区间（buffer 行范围）→ 流行范围（fold 拓扑的输入行空间）。
fn hidden_spans_in_stream(stream: &LineStream, folds: &SumTree<Fold>) -> Vec<Range<usize>> {
    hidden_spans(folds)
        .into_iter()
        .map(|span| {
            stream.buffer_to_stream(Line::new(span.start)).get()
                ..stream.buffer_to_stream(Line::new(span.end)).get()
        })
        .collect()
}

fn build_transforms(spans: &[Range<usize>], line_count: usize) -> SumTree<Transform> {
    let mut transforms = Vec::new();
    let mut line = 0;
    for span in spans {
        if line < span.start {
            transforms.push(Transform::isomorphic(span.start - line));
        }
        transforms.push(Transform::fold(span.end - span.start));
        line = span.end;
    }
    if line < line_count {
        transforms.push(Transform::isomorphic(line_count - line));
    }
    SumTree::from_iter(transforms, ())
}

fn full_fold_edit(old_rows: usize, new_rows: usize) -> FoldEdit {
    FoldEdit {
        old: ProjectedLineIndex::ZERO..ProjectedLineIndex::new(old_rows),
        new: ProjectedLineIndex::ZERO..ProjectedLineIndex::new(new_rows),
        changed_lines: Vec::new(),
        structural: true,
    }
}

fn inline_fold_edits(
    batch: &TextChangeBatch,
    stream: &LineStream,
    folds: &SumTree<Fold>,
) -> Vec<FoldEdit> {
    batch
        .patch()
        .edits()
        .iter()
        .filter_map(|edit| {
            let buffer = stream.buffer_snapshot();
            let start = buffer.byte_to_line(edit.new_range().start()).ok()?;
            let end = buffer.byte_to_line(edit.new_range().end()).ok()?;
            // changed_lines 是流行号（下游缓存失效按流行）。
            //
            // 折叠覆盖行（anchor 与 close 之间的隐藏行、close 行）的编辑映射到最外层折叠的 anchor 行：
            // 隐藏行编辑不改变显示文本；close 行编辑改变合并行尾段，合并行必须重排。
            // anchor 行自身可见，其编辑已在行列表中，不映射。
            let changed_lines = (start.get()..=end.get())
                .map(|line| {
                    let stream_line = stream.buffer_to_stream(Line::new(line));
                    folds
                        .iter()
                        .filter(|fold| {
                            let (span_start, span_end) = fold.line_span;
                            span_start.get() < line && line <= span_end.get()
                        })
                        .min_by_key(|fold| fold.line_span.0)
                        .map_or(stream_line, |fold| {
                            stream.buffer_to_stream(fold.line_span.0)
                        })
                })
                .collect();
            Some(FoldEdit {
                old: ProjectedLineIndex::ZERO..ProjectedLineIndex::ZERO,
                new: ProjectedLineIndex::ZERO..ProjectedLineIndex::ZERO,
                changed_lines,
                structural: false,
            })
        })
        .collect()
}

/// 折叠行范围的折叠区间：从入口行行尾换行符到末行内容末尾（换行符除外）。
///
/// 对齐 Zed：入口行换行符被折叠吞掉（占位符与闭合尾段拼在同一显示行），末行换行符留在折叠外（占位符行以该换行符结束）。
fn text_range_for_folded_lines(
    snapshot: &Snapshot,
    lines: LineRange,
) -> DisplayMapResult<TextRange> {
    let start = line_newline_position(snapshot, lines.start())?;
    let end = line_content_end(snapshot, Line::new(lines.end().get() - 1))?;
    if start >= end {
        return Err(FoldError::EmptyRange {
            range: TextRange::new(start, end)?,
        }
        .into());
    }
    Ok(TextRange::new(start, end)?)
}

/// 行终止换行符（`\n`）的字节位置；行尾无换行符时返回行尾。
fn line_newline_position(snapshot: &Snapshot, line: Line) -> DisplayMapResult<ByteOffset> {
    let content = snapshot.line_content(line, None)?;
    if content.text_range().end() == content.full_range().end() {
        Ok(content.full_range().end())
    } else {
        // 行含终止换行符：`\r?\n` 的 `\n` 位于行尾前一字节。
        Ok(ByteOffset::new(content.full_range().end().get() - 1))
    }
}

/// 行内容末尾（不含终止换行符）。
fn line_content_end(snapshot: &Snapshot, line: Line) -> DisplayMapResult<ByteOffset> {
    Ok(snapshot.line_content(line, None)?.text_range().end())
}

/// 折叠范围的逻辑行跨度：起点行（anchor）与终点所在行（close，含隐藏前缀）。
///
/// 折叠范围是字节级的（终点在 close 行内），终点行即被折叠的 close 行，不再按"终点恰在行首"回退（行首终点只可能来自旧的整行折叠形状）。
fn fold_line_span(snapshot: &Snapshot, range: TextRange) -> DisplayMapResult<(Line, Line)> {
    let start = snapshot.byte_to_line(range.start())?;
    let end = snapshot.byte_to_line(range.end())?;
    Ok((start, end))
}

fn ranges_disjoint_or_nested(left: TextRange, right: TextRange) -> bool {
    left.end() <= right.start()
        || right.end() <= left.start()
        || (left.start() <= right.start() && right.end() <= left.end())
        || (right.start() <= left.start() && left.end() <= right.end())
}

#[cfg(test)]
mod tests {
    use zcv_engine::{Buffer, BufferConfig};

    use super::super::inlay_map::InlayMap;
    use super::*;

    fn line_range(start: usize, end: usize) -> LineRange {
        LineRange::new(Line::new(start), Line::new(end)).unwrap()
    }

    fn text_range(start: usize, end: usize) -> TextRange {
        TextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap()
    }

    #[test]
    fn fold_snapshot_owns_fold_and_transform_trees_and_keeps_old_snapshots_stable() {
        let buffer = Buffer::scratch(
            "anchor\nhidden one\nhidden two\nafter".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let (mut map, before) = FoldMap::new(InlayMap::new(LineStream::new(buffer.snapshot())).1);
        let (after, edits) = map
            .write()
            .fold_lines(LineRange::new(Line::ZERO, Line::new(3)).unwrap())
            .unwrap();

        assert_eq!(before.line_count(), 4);
        assert_eq!(after.line_count(), 2);
        assert_eq!(
            before.buffer_snapshot().version(),
            after.buffer_snapshot().version()
        );
        assert_ne!(before.version(), after.version());
        assert_eq!(after.folds.summary().count, 1);
        assert!(edits.iter().all(FoldEdit::is_structural));
    }

    #[test]
    fn fold_writer_rejects_partial_overlap_but_accepts_nesting() {
        let buffer = Buffer::scratch("abcdef".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(LineStream::new(buffer.snapshot())).1);
        map.write().fold(text_range(1, 5)).unwrap();
        map.write().fold(text_range(2, 4)).unwrap();

        let error = map.write().fold(text_range(0, 3)).unwrap_err();
        assert!(matches!(
            error,
            super::super::error::DisplayMapError::Fold(FoldError::OverlapWithoutNesting { .. })
        ));
        assert_eq!(map.snapshot.folds.summary().count, 2);
    }

    #[test]
    fn unfolding_outer_fold_reveals_the_nested_transform() {
        let buffer = Buffer::scratch("a\nb\nc\nd\ne".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(LineStream::new(buffer.snapshot())).1);
        map.write().fold_lines(line_range(0, 4)).unwrap();
        map.write().fold_lines(line_range(1, 3)).unwrap();
        let outer = map
            .snapshot
            .folds
            .iter()
            .min_by_key(|fold| fold.text_range().start())
            .unwrap()
            .id;

        assert_eq!(map.snapshot.line_count(), 2);
        let (snapshot, edits) = map.write().unfold(outer);
        assert_eq!(snapshot.folds.summary().count, 1);
        assert_eq!(snapshot.line_count(), 4);
        assert!(edits[0].is_structural());
    }

    #[test]
    fn inline_edit_advances_fold_snapshot_without_rebuilding_transforms() {
        let mut buffer =
            Buffer::scratch("anchor\nhidden\nafter".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(LineStream::new(buffer.snapshot())).1);
        map.write().fold_lines(line_range(0, 2)).unwrap();
        let transforms = map.snapshot.transforms.clone();
        let subscription = buffer.subscribe();
        buffer.insert(ByteOffset::new(9), "!").unwrap();

        let (snapshot, edits, outcome) = map.read(
            InlayMap::new(LineStream::new(buffer.snapshot())).1,
            &subscription.consume(),
        );
        assert_eq!(outcome, ApplyOutcome::Compatible);
        assert_eq!(snapshot.transforms, transforms);
        assert_eq!(snapshot.folds.summary().count, 1);
        assert!(edits.iter().all(|edit| !edit.is_structural()));
    }

    #[test]
    fn newline_edit_rebuilds_transform_tree_and_emits_structural_fold_edit() {
        let mut buffer =
            Buffer::scratch("anchor\nhidden\nafter".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(LineStream::new(buffer.snapshot())).1);
        map.write().fold_lines(line_range(0, 2)).unwrap();
        let subscription = buffer.subscribe();
        buffer.insert(ByteOffset::new(9), "new\n").unwrap();

        let (snapshot, edits, outcome) = map.read(
            InlayMap::new(LineStream::new(buffer.snapshot())).1,
            &subscription.consume(),
        );
        assert_eq!(outcome, ApplyOutcome::Spliced);
        assert_eq!(
            snapshot.logical_line_count(),
            buffer.snapshot().line_count()
        );
        assert!(edits.iter().all(FoldEdit::is_structural));
    }

    #[test]
    fn deleting_folded_text_invalidates_tracked_fold() {
        let mut buffer =
            Buffer::scratch("anchor\nhidden\nafter".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(LineStream::new(buffer.snapshot())).1);
        map.write().fold_lines(line_range(0, 2)).unwrap();
        let subscription = buffer.subscribe();
        buffer
            .delete(text_range(0, "anchor\nhidden\n".len()))
            .unwrap();

        let (snapshot, _, _) = map.read(
            InlayMap::new(LineStream::new(buffer.snapshot())).1,
            &subscription.consume(),
        );
        assert_eq!(snapshot.folds.summary().count, 0);
        assert_eq!(
            snapshot.line_count(),
            snapshot.buffer_snapshot().line_count()
        );
    }

    #[test]
    fn folded_points_map_through_anchor_in_both_directions() {
        let buffer = Buffer::scratch("a\nb\nc\nd".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(LineStream::new(buffer.snapshot())).1);
        let (snapshot, _) = map.write().fold_lines(line_range(0, 3)).unwrap();

        // 折叠段不产生投影行：行数 = 4 - 2 隐藏 = 2。
        assert_eq!(snapshot.line_count(), 2);
        // 隐藏行按 bias 吸附到合并行（anchor 行 0）的折叠起点/终点列；
        // anchor 行 "a" 内容 1 字符，占位符 1 字符。
        let left = snapshot
            .logical_to_projected_point(LogicalPoint::line_start(Line::new(1)), FoldBias::Left)
            .unwrap();
        assert_eq!(
            left,
            ProjectedPoint::new(ProjectedLineIndex::new(0), LogicalColumn::new(1))
        );
        let right = snapshot
            .logical_to_projected_point(LogicalPoint::line_start(Line::new(1)), FoldBias::Right)
            .unwrap();
        assert_eq!(
            right,
            ProjectedPoint::new(ProjectedLineIndex::new(0), LogicalColumn::new(2))
        );
        // 投影行 1 是可见文本行（"d"）。
        let text = snapshot
            .projected_line_kind(ProjectedLineIndex::new(1))
            .unwrap();
        assert_eq!(text.logical_line(), Line::new(3));
    }
}

/// FoldSnapshot 中投影行的 0-indexed 索引。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ProjectedLineIndex(usize);

impl ProjectedLineIndex {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
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
/// `column` 与对应逻辑行的 `LogicalColumn` 同义（投影行均为可见文本行）。
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

/// 折叠合并行（anchor 行文本 + 占位符 + 闭合行尾段）的投影几何。
#[derive(Debug, Clone, Copy)]
struct FoldMergedGeometry {
    /// 合并行的投影行号（anchor 行）。
    row: ProjectedLineIndex,
    /// anchor 段字符数（含行内提示注入，不含行尾换行）。
    anchor_chars: usize,
    /// 闭合行（折叠范围终点所在行）。
    close_line: Line,
    /// 尾段起点：闭合行内原始字节偏移（折叠范围终点 - 行首）。
    tail_byte: usize,
    /// 尾段起点：闭合行内投影偏移（含行内提示注入）。
    tail_projected: usize,
    /// 尾段起点：闭合行原始列（隐藏前缀的字符数）。
    tail_start_col: usize,
}

/// 逻辑文档内的有序点对范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LogicalRange {
    start: LogicalPoint,
    end: LogicalPoint,
}

impl LogicalRange {
    /// 要求 `start <= end`（按 line, column 字典序）。
    pub fn new(start: LogicalPoint, end: LogicalPoint) -> Result<Self, CoordinateError> {
        if !is_ordered_logical(start, end) {
            return Err(CoordinateError::InvalidLineRange {
                start: start.line,
                end: end.line,
            });
        }
        Ok(Self { start, end })
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
}

/// 投影空间内的有序点对范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectedRange {
    start: ProjectedPoint,
    end: ProjectedPoint,
}

impl ProjectedRange {
    /// 要求 `start <= end`（按 projected line, column 字典序）。
    pub fn new(start: ProjectedPoint, end: ProjectedPoint) -> Result<Self, CoordinateError> {
        if !is_ordered_projected(start, end) {
            return Err(CoordinateError::InvalidLineRange {
                start: Line::new(start.line.get()),
                end: Line::new(end.line.get()),
            });
        }
        Ok(Self { start, end })
    }

    pub const fn start(self) -> ProjectedPoint {
        self.start
    }

    pub const fn end(self) -> ProjectedPoint {
        self.end
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
