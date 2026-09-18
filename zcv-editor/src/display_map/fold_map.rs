//! 折叠显示层。
//!
//! FoldMap 是唯一写入口，向上层发布 FoldSnapshot 与 FoldEdit。
//!
//! 折叠模型：折叠范围是字节级的（入口行行尾换行符 → 闭合括号前），折叠段不产生投影行，占位符文本拼入 anchor 行的合并行（anchor 全文 + 占位符 +闭合行尾段），因此闭合括号保留为真实可见文本。
//! 隐藏点投影遵循 bias 约定（Left 吸附折叠起点列，Right 吸附折叠终点列）。

use zcv_multi_buffer::{MultiBufferAnchor, MultiBufferOffset, MultiBufferRange};

use std::{borrow::Cow, cmp::Reverse, collections::BTreeMap, ops::Range};

use sum_tree::{Bias as TreeBias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use zcv_multi_buffer::MultiBufferSnapshot;
use zcv_text::{Affinity, CoordinateError, Line, LineRange, LogicalColumn, Position};

use super::edit::ProjectionEdit;
use super::error::{DisplayMapResult, FoldError};
use super::inlay_map::{InlayEdit, InlaySnapshot};
use super::tab_map::line_content;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct FoldId(u64);

impl FoldId {
    const INITIAL: Self = Self(1);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Fold {
    id: FoldId,
    /// 折叠端点的长期表示：组合锚点，跨文本编辑与投影重建由当前快照解析。
    range: Range<MultiBufferAnchor>,
    /// 当前快照下解析出的组合字节范围（派生缓存，随同步刷新）；折叠查询与拓扑索引都读它。
    text_range: MultiBufferRange,
    line_span: (Line, Line),
}

impl Fold {
    /// 用当前快照把一段组合字节范围折叠为锚点表示。
    fn from_text_range(
        snapshot: &MultiBufferSnapshot,
        id: FoldId,
        range: MultiBufferRange,
    ) -> Option<Self> {
        let line_span = fold_line_span(snapshot, range).ok()?;
        Some(Self {
            id,
            // range_inside 语义：起点贴插入之后、终点贴插入之前，折叠不吸收边界插入。
            range: snapshot.anchor_at(range.start(), Affinity::After)
                ..snapshot.anchor_at(range.end(), Affinity::Before),
            text_range: range,
            line_span,
        })
    }

    /// 用新快照按锚点重新解析折叠范围；锚点已退出投影或范围退化时返回 None。
    fn resolve(&self, snapshot: &MultiBufferSnapshot) -> Option<Self> {
        let start = snapshot.resolve_anchor(&self.range.start)?;
        let end = snapshot.resolve_anchor(&self.range.end)?;
        let range = MultiBufferRange::new(start, end).ok()?;
        let line_span = fold_line_span(snapshot, range).ok()?;
        Some(Self {
            id: self.id,
            range: self.range.clone(),
            text_range: range,
            line_span,
        })
    }

    fn text_range(&self) -> MultiBufferRange {
        self.text_range
    }
}

impl Item for Fold {
    type Summary = FoldSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        FoldSummary {
            count: 1,
            last_order: FoldOrder::for_fold(self),
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
    fn for_fold(fold: &Fold) -> Self {
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

/// 隐藏点投影的 bias 约定：Left 吸附折叠起点列，Right 吸附折叠终点列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoldBias {
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
            // 折叠段完全吞掉输入行，不产生投影行；占位符拼入 anchor 行的合并行文本。
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

/// InlayEdit 还原到 buffer 坐标后的一段编辑，供 fold 拓扑定位使用。
type FoldBufferEdit = ProjectionEdit<MultiBufferOffset>;

impl FoldEdit {
    pub(super) fn changed_lines(&self) -> &[Line] {
        &self.changed_lines
    }

    pub(super) fn is_structural(&self) -> bool {
        self.structural || self.old.start != self.new.start || self.old.end != self.new.end
    }

    /// 旧子树的投影输入行区间（tab 输入行）；结构编辑中被替换的部分。
    pub(super) fn old_rows(&self) -> Range<usize> {
        self.old.start.get()..self.old.end.get()
    }

    /// 新子树的投影输入行区间（tab 输入行）；结构编辑中替换后的部分。
    pub(super) fn new_rows(&self) -> Range<usize> {
        self.new.start.get()..self.new.end.get()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FoldSnapshot {
    /// 输入投影快照（行内提示注入后的流）：fold 拓扑工作在其上，外部文本可被折叠。
    input: InlaySnapshot,
    folds: SumTree<Fold>,
    lookup: FoldLookup,
    transforms: SumTree<Transform>,
    fold_metadata_by_id: BTreeMap<FoldId, MultiBufferRange>,
    version: u64,
}

/// 折叠查询索引。
/// `by_start` 按起点排序，前缀最大终点允许覆盖查询二分定位；
/// `anchor_by_line` 只保存未被外层折叠遮蔽的可见入口行。
#[derive(Debug, Clone, Default)]
struct FoldLookup {
    by_start: Vec<Fold>,
    prefix_max_end: Vec<usize>,
    prefix_fold_index: Vec<usize>,
    prefix_max_line_end: Vec<usize>,
    prefix_line_fold_index: Vec<usize>,
    anchor_by_line: BTreeMap<Line, Fold>,
}

impl FoldLookup {
    fn from_folds(folds: &[Fold]) -> Self {
        let mut by_start = folds.to_vec();
        by_start.sort_by_key(FoldOrder::for_fold);
        let mut prefix_max_end = Vec::with_capacity(by_start.len());
        let mut prefix_fold_index = Vec::with_capacity(by_start.len());
        let mut prefix_max_line_end = Vec::with_capacity(by_start.len());
        let mut prefix_line_fold_index = Vec::with_capacity(by_start.len());
        let mut max_end = 0;
        let mut max_index = 0;
        let mut max_line_end = 0;
        let mut max_line_index = 0;
        let mut active_ends = Vec::new();
        let mut anchor_by_line = BTreeMap::new();
        for (index, fold) in by_start.iter().enumerate() {
            let range = fold.text_range();
            let start = range.start().get();
            let end = range.end().get();
            if end > max_end {
                max_end = end;
                max_index = index;
            }
            prefix_max_end.push(max_end);
            prefix_fold_index.push(max_index);
            let line_end = fold.line_span.1.get();
            if line_end > max_line_end {
                max_line_end = line_end;
                max_line_index = index;
            }
            prefix_max_line_end.push(max_line_end);
            prefix_line_fold_index.push(max_line_index);

            active_ends.retain(|active_end| *active_end >= start);
            if fold.line_span.0 < fold.line_span.1 && active_ends.is_empty() {
                anchor_by_line
                    .entry(fold.line_span.0)
                    .or_insert_with(|| fold.clone());
            }
            active_ends.push(end);
        }
        Self {
            by_start,
            prefix_max_end,
            prefix_fold_index,
            prefix_max_line_end,
            prefix_line_fold_index,
            anchor_by_line,
        }
    }

    fn covering_offset(&self, offset: MultiBufferOffset) -> Option<&Fold> {
        let upper = self
            .by_start
            .partition_point(|fold| fold.text_range().start() <= offset);
        if upper == 0 {
            return None;
        }
        let index = self.prefix_max_end[..upper].partition_point(|end| *end <= offset.get());
        self.prefix_fold_index
            .get(index)
            .and_then(|fold_index| self.by_start.get(*fold_index))
    }

    fn covering_line(&self, line: Line) -> Option<&Fold> {
        let upper = self
            .by_start
            .partition_point(|fold| fold.line_span.0 < line);
        if upper == 0 {
            return None;
        }
        let index = self.prefix_max_line_end[..upper].partition_point(|end| *end < line.get());
        self.prefix_line_fold_index
            .get(index)
            .and_then(|fold_index| self.by_start.get(*fold_index))
    }
}

impl FoldSnapshot {
    pub(crate) fn inlay_snapshot(&self) -> &InlaySnapshot {
        &self.input
    }

    pub(super) fn buffer_snapshot(&self) -> &MultiBufferSnapshot {
        self.input.buffer_snapshot()
    }

    pub(super) const fn version(&self) -> u64 {
        self.version
    }

    pub(super) fn line_count(&self) -> usize {
        self.transforms.summary().output_rows
    }

    /// 覆盖该字节偏移的最外层折叠的隐藏范围（`[入口行换行符, 闭合括号前)`）；无则 None。
    ///
    /// 水平移动用：目标落在折叠内时按方向吸附到折叠终点/起点（折叠在显示上占一个字符）。
    pub(crate) fn fold_range_covering_offset(
        &self,
        offset: MultiBufferOffset,
    ) -> Option<(MultiBufferOffset, MultiBufferOffset)> {
        self.lookup.covering_offset(offset).map(|fold| {
            let range = fold.text_range();
            (range.start(), range.end())
        })
    }

    /// 折叠入口行（合并行占位符的挂靠行；无隐藏行的 fold 不计）。
    pub(crate) fn fold_anchor_lines(&self) -> Vec<Line> {
        self.folds
            .iter()
            .filter_map(|fold| {
                let (start, end) = fold.line_span;
                (start < end).then_some(start)
            })
            .collect()
    }

    pub(super) fn fold_anchor_lines_in_range(&self, line_range: Range<Line>) -> Vec<Line> {
        self.lookup
            .anchor_by_line
            .range(line_range)
            .map(|(line, _)| *line)
            .collect()
    }

    fn logical_line_count(&self) -> usize {
        // fold 拓扑的输入行 = 流行（buffer + 合成）。
        self.transforms.summary().input_lines
    }

    pub(crate) fn projected_line_kind(&self, index: ProjectedLineIndex) -> Option<TextLine> {
        if index.get() >= self.line_count() {
            return None;
        }
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
            TransformKind::Fold => LogicalProjection::Hidden,
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
            LogicalProjection::Hidden => unreachable!("fold_covering 已覆盖全部隐藏行"),
        }
    }

    /// 覆盖该逻辑行的最外层折叠（anchor 与 close 之间的隐藏行，含 close 行）。
    fn fold_covering(&self, line: Line) -> Option<&Fold> {
        self.lookup.covering_line(line)
    }

    /// 折叠合并行的投影几何（anchor 行文本 + 占位符 + 闭合行尾段）。
    fn fold_merged_geometry(&self, fold: &Fold) -> DisplayMapResult<FoldMergedGeometry> {
        let anchor = fold.line_span.0;
        let row = match self.logical_to_projected(anchor)? {
            LogicalProjection::Visible(row) => row,
            LogicalProjection::Hidden => unreachable!("折叠 anchor 行必须可见"),
        };
        let inlay = self.inlay_snapshot();
        let anchor_stream = anchor;
        let (anchor_len, anchor_chars) = inlay
            .projected_line_content_metrics(anchor_stream)
            .expect("折叠 anchor 行必须位于流内");
        let close_line = fold.line_span.1;
        let close_stream = close_line;
        let close_start = self
            .buffer_snapshot()
            .line_start_byte(close_line)
            .expect("折叠 close 行必须位于当前 Snapshot 内");
        let tail_projected = inlay.to_projected_offset(
            close_stream,
            fold.text_range().end().get() - close_start.get(),
        );
        let content_end_projected = inlay
            .projected_line_content_metrics(close_stream)
            .expect("折叠 close 行必须位于流内")
            .0;
        let tail_start_col = self
            .buffer_snapshot()
            .byte_to_position(fold.text_range().end())
            .ok()
            .map_or(0, |position| position.column().get());
        Ok(FoldMergedGeometry {
            row,
            anchor_stream,
            anchor_chars,
            anchor_len,
            close_line,
            close_stream,
            tail_projected,
            content_end_projected,
            tail_start_col,
        })
    }

    /// 投影行 → 是否折叠合并行（可见 anchor 行）。
    pub(crate) fn is_fold_row(&self, row: ProjectedLineIndex) -> bool {
        self.fold_for_row(row).is_some()
    }

    /// 投影行 → 折叠合并行的 anchor 行（流行号）。
    pub(crate) fn fold_row_anchor_stream_line(&self, row: ProjectedLineIndex) -> Option<Line> {
        self.fold_for_row(row).map(|fold| fold.line_span.0)
    }

    /// 投影行 → 折叠合并行的段表（合并文本字节空间的切分，文本顺序）。
    pub(crate) fn fold_row_segments(&self, row: ProjectedLineIndex) -> Option<[FoldRowSegment; 3]> {
        let fold = self.fold_for_row(row)?;
        let geometry = self.fold_merged_geometry(fold).ok()?;
        let tail_len = geometry.content_end_projected - geometry.tail_projected;
        let tail_start = geometry.anchor_len + FOLD_PLACEHOLDER.len();
        Some([
            FoldRowSegment {
                merged_range: 0..geometry.anchor_len,
                kind: FoldRowSegmentKind::Text {
                    stream_line: geometry.anchor_stream,
                    projected_range: 0..geometry.anchor_len,
                },
            },
            FoldRowSegment {
                merged_range: geometry.anchor_len..tail_start,
                kind: FoldRowSegmentKind::Placeholder,
            },
            FoldRowSegment {
                merged_range: tail_start..tail_start + tail_len,
                kind: FoldRowSegmentKind::Text {
                    stream_line: geometry.close_stream,
                    projected_range: geometry.tail_projected..geometry.content_end_projected,
                },
            },
        ])
    }

    /// 投影行 → 行文本；折叠合并行为 anchor 全文 + 占位符 + 闭合行尾段。
    pub(crate) fn row_text(&self, row: ProjectedLineIndex) -> Option<Cow<'_, str>> {
        if let Some(fold) = self.fold_for_row(row) {
            let geometry = self.fold_merged_geometry(fold).ok()?;
            let anchor = self.input.line_text(geometry.anchor_stream)?;
            let close = self.input.line_text(geometry.close_stream)?;
            let mut text = String::with_capacity(
                geometry.anchor_len
                    + FOLD_PLACEHOLDER.len()
                    + geometry
                        .content_end_projected
                        .saturating_sub(geometry.tail_projected)
                    + 1,
            );
            text.push_str(line_content(anchor.as_ref()));
            text.push_str(FOLD_PLACEHOLDER);
            text.push_str(&close.as_ref()[geometry.tail_projected..geometry.content_end_projected]);
            text.push('\n');
            return Some(Cow::Owned(text));
        }
        let line = self.projected_line_kind(row)?.logical_line();
        self.input.line_text(line)
    }

    /// 折叠对应的可见 anchor 行（未被外层折叠覆盖），供合并行查询。
    fn fold_for_row(&self, row: ProjectedLineIndex) -> Option<&Fold> {
        let text = self.projected_line_kind(row)?;
        let buffer_line = self.input.source(text.logical_line())?;
        self.lookup.anchor_by_line.get(&buffer_line)
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
        let projected = self
            .inlay_snapshot()
            .to_projected_offset(geometry.close_line, byte - close_start.get());
        Ok(projected - geometry.tail_projected)
    }

    /// 投影行的内容来源：fold 投影（Text）叠加流行解析。
    ///
    /// 文本行统一携带对应的 buffer 行来源。
    pub(super) fn projected_kind(
        &self,
        projected: ProjectedLineIndex,
    ) -> Option<StreamProjectedKind> {
        let text = self.projected_line_kind(projected)?;
        Some(StreamProjectedKind::Text(
            self.input.source(text.logical_line())?,
        ))
    }
}

/// 投影行（fold 输出）的内容来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamProjectedKind {
    /// 可见文本行及其 buffer 行来源。
    Text(Line),
}

#[derive(Debug, Clone)]
pub(super) struct FoldMap {
    snapshot: FoldSnapshot,
    next_fold_id: FoldId,
}

impl FoldMap {
    pub(super) fn new(input: InlaySnapshot) -> (Self, FoldSnapshot) {
        let transforms = build_transforms(&[], input.line_count());
        let snapshot = FoldSnapshot {
            input,
            folds: SumTree::new(()),
            lookup: FoldLookup::default(),
            transforms,
            fold_metadata_by_id: BTreeMap::new(),
            version: 0,
        };
        (
            Self {
                snapshot: snapshot.clone(),
                next_fold_id: FoldId::INITIAL,
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
        inlay_edits: Vec<InlayEdit>,
    ) -> (FoldSnapshot, Vec<FoldEdit>) {
        let buffer = input.buffer_snapshot().clone();
        let old_buffer = self.snapshot.buffer_snapshot().clone();
        let old_inlay = self.snapshot.input.clone();
        // 注入配置变化（inlay 增删改）不产生 buffer 编辑：整体重建 fold 拓扑。
        let inlay_changed = input.version() != old_inlay.version();
        if buffer.version() == old_buffer.version() && !inlay_changed {
            // 文本与 inlay 结构未变，但捕获表或元数据可能已更新：
            // 采用新输入快照，保证 fold 链上仍持有当前 MultiBufferSnapshot。
            self.snapshot.input = input;
            return (self.snapshot.clone(), Vec::new());
        }

        if inlay_changed {
            let old_rows = self.snapshot.line_count();
            let stream_line_count = input.line_count();
            self.snapshot = FoldSnapshot {
                transforms: build_transforms(&[], stream_line_count),
                input,
                folds: self.snapshot.folds.clone(),
                lookup: self.snapshot.lookup.clone(),
                fold_metadata_by_id: self.snapshot.fold_metadata_by_id.clone(),
                version: self.snapshot.version + 1,
            };
            let edit = full_fold_edit(old_rows, self.snapshot.line_count());
            return (self.snapshot.clone(), vec![edit]);
        }

        // InlayEdit 位于 inlay 坐标空间；用旧/新 inlay 快照还原到 buffer 坐标，
        // 供 fold 拓扑与下游换行定位使用。fold 的长期端点保存 Anchor，直接按当前快照解析，不读 PositionMap。
        let buffer_edits: Vec<FoldBufferEdit> = inlay_edits
            .iter()
            .map(|edit| {
                ProjectionEdit::new(
                    old_inlay.to_buffer_offset(edit.old.start)
                        ..old_inlay.to_buffer_offset(edit.old.end),
                    input.to_buffer_offset(edit.new.start)..input.to_buffer_offset(edit.new.end),
                )
            })
            .collect();

        let old_rows = self.snapshot.line_count();
        let old_spans = hidden_spans(&self.snapshot.folds);
        let mut retained = Vec::new();
        self.snapshot.fold_metadata_by_id.clear();
        // 活动折叠只保存组合锚点：编辑后按新快照重新解析，不再手工重映射裸偏移。
        for fold in self.snapshot.folds.iter().cloned() {
            let Some(fold) = fold.resolve(&buffer) else {
                continue;
            };
            // 锚点被删除吞并后退化为空范围时不再保留。
            if fold.text_range().is_empty() {
                continue;
            }
            self.snapshot
                .fold_metadata_by_id
                .insert(fold.id, fold.text_range());
            retained.push(fold);
        }
        sort_folds(&mut retained);
        self.snapshot.lookup = FoldLookup::from_folds(&retained);
        self.snapshot.folds = SumTree::from_iter(retained, ());
        let new_spans = hidden_spans(&self.snapshot.folds);
        // 行数不变且折叠拓扑不变时，编辑只改变与编辑区间相交行的内容，`inline_fold_edits` 的 changed_lines 恰好覆盖；
        // 软换行的逐行重排（update_inline）对行数不变的多行编辑同样正确。
        let structural = old_spans != new_spans || old_buffer.line_count() != buffer.line_count();
        self.snapshot.input = input;
        self.snapshot.version += 1;
        if structural {
            let spans = hidden_spans_in_stream(&self.snapshot.input, &self.snapshot.folds);
            self.snapshot.transforms = build_transforms(&spans, self.snapshot.input.line_count());
        }
        let edits = if structural {
            linear_fold_edit(&buffer_edits, &old_buffer, &buffer, &old_spans, &new_spans)
                .map_or_else(
                    || vec![full_fold_edit(old_rows, self.snapshot.line_count())],
                    |edit| vec![edit],
                )
        } else {
            inline_fold_edits(&buffer_edits, &self.snapshot.input, &self.snapshot.folds)
        };
        (self.snapshot.clone(), edits)
    }

    pub(super) fn write(&mut self) -> FoldMapWriter<'_> {
        FoldMapWriter(self)
    }
}

pub(super) struct FoldMapWriter<'a>(&'a mut FoldMap);

impl FoldMapWriter<'_> {
    /// 展开与行范围交叠的全部折叠（半开区间）。
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

    pub(super) fn fold(
        &mut self,
        range: Range<MultiBufferAnchor>,
    ) -> DisplayMapResult<(FoldSnapshot, Vec<FoldEdit>)> {
        let resolved = {
            let snapshot = self.0.snapshot.buffer_snapshot();
            let start = snapshot
                .resolve_anchor(&range.start)
                .ok_or(FoldError::UnresolvableAnchor)?;
            let end = snapshot
                .resolve_anchor(&range.end)
                .ok_or(FoldError::UnresolvableAnchor)?;
            MultiBufferRange::new(start, end)?
        };
        if resolved.is_empty() {
            return Err(FoldError::EmptyRange { range: resolved }.into());
        }
        if self
            .0
            .snapshot
            .folds
            .iter()
            .any(|fold| fold.text_range() == resolved)
        {
            return Ok((self.0.snapshot.clone(), Vec::new()));
        }
        for fold in self.0.snapshot.folds.iter() {
            if !ranges_disjoint_or_nested(fold.text_range(), resolved) {
                return Err(FoldError::OverlapWithoutNesting {
                    existing: fold.text_range(),
                    candidate: resolved,
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
        let stream_line_count = self.0.snapshot.input.line_count();
        let old_spans = hidden_spans_in_stream(&self.0.snapshot.input, &self.0.snapshot.folds);
        let mut folds: Vec<_> = self.0.snapshot.folds.iter().cloned().collect();
        let fold = Fold::from_text_range(self.0.snapshot.buffer_snapshot(), id, resolved)
            .ok_or(FoldError::UnresolvableAnchor)?;
        folds.push(fold);
        sort_folds(&mut folds);
        self.0.snapshot.folds = SumTree::from_iter(folds, ());
        let indexed_folds = self.0.snapshot.folds.iter().cloned().collect::<Vec<_>>();
        self.0.snapshot.lookup = FoldLookup::from_folds(&indexed_folds);
        self.0.snapshot.fold_metadata_by_id.insert(id, resolved);
        let spans = hidden_spans_in_stream(&self.0.snapshot.input, &self.0.snapshot.folds);
        self.0.snapshot.transforms = build_transforms(&spans, stream_line_count);
        self.0.snapshot.version += 1;
        Ok((
            self.0.snapshot.clone(),
            span_edit(&old_spans, &spans).into_iter().collect(),
        ))
    }

    fn unfold(&mut self, id: FoldId) -> (FoldSnapshot, Vec<FoldEdit>) {
        if !self.0.snapshot.fold_metadata_by_id.contains_key(&id) {
            return (self.0.snapshot.clone(), Vec::new());
        }
        let stream_line_count = self.0.snapshot.input.line_count();
        let old_spans = hidden_spans_in_stream(&self.0.snapshot.input, &self.0.snapshot.folds);
        let retained: Vec<_> = self
            .0
            .snapshot
            .folds
            .iter()
            .filter(|&fold| fold.id != id)
            .cloned()
            .collect();
        self.0.snapshot.folds = SumTree::from_iter(retained, ());
        let indexed_folds = self.0.snapshot.folds.iter().cloned().collect::<Vec<_>>();
        self.0.snapshot.lookup = FoldLookup::from_folds(&indexed_folds);
        self.0.snapshot.fold_metadata_by_id.remove(&id);
        let spans = hidden_spans_in_stream(&self.0.snapshot.input, &self.0.snapshot.folds);
        self.0.snapshot.transforms = build_transforms(&spans, stream_line_count);
        self.0.snapshot.version += 1;
        (
            self.0.snapshot.clone(),
            span_edit(&old_spans, &spans).into_iter().collect(),
        )
    }
}

fn sort_folds(folds: &mut [Fold]) {
    folds.sort_by_key(FoldOrder::for_fold);
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
fn hidden_spans_in_stream(inlay: &InlaySnapshot, folds: &SumTree<Fold>) -> Vec<Range<usize>> {
    let _ = inlay;
    hidden_spans(folds)
        .into_iter()
        .map(|span| span.start..span.end)
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

/// 由隐藏跨度差异派生**局部**结构编辑：只覆盖 old/new 跨度真正不同的区间。
///
/// 跨度在 stream 行空间（投影行前缀由累计隐藏行数决定）。前后公共跨度的投影行
/// 数量相同，因此编辑两端在旧/新拓扑中落在同一投影行，只有中间区间需要被 Wrap 重排。
/// 完全相同（折叠被内层/外层吞并等）时返回 None，上游无需重排。
fn span_edit(old_spans: &[Range<usize>], new_spans: &[Range<usize>]) -> Option<FoldEdit> {
    let mut prefix = 0;
    while prefix < old_spans.len()
        && prefix < new_spans.len()
        && old_spans[prefix] == new_spans[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old_spans.len() - prefix
        && suffix < new_spans.len() - prefix
        && old_spans[old_spans.len() - 1 - suffix] == new_spans[new_spans.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let old_middle = &old_spans[prefix..old_spans.len() - suffix];
    let new_middle = &new_spans[prefix..new_spans.len() - suffix];
    if old_middle == new_middle {
        return None;
    }
    // 变更区间只看中间跨度：公共前缀/后缀之外、真正被替换的 stream 行区间。
    let region_start = match (old_middle.first(), new_middle.first()) {
        (Some(old_span), Some(new_span)) => old_span.start.min(new_span.start),
        (Some(old_span), None) => old_span.start,
        (None, Some(new_span)) => new_span.start,
        (None, None) => return None,
    };
    let region_end = match (old_middle.last(), new_middle.last()) {
        (Some(old_span), Some(new_span)) => old_span.end.max(new_span.end),
        (Some(old_span), None) => old_span.end,
        (None, Some(new_span)) => new_span.end,
        (None, None) => return None,
    };
    let hidden_before = hidden_lines(&old_spans[..prefix]);
    // 公共前缀在旧/新拓扑中投影行数相同：区间起点在两种拓扑里落在同一 tab 行。
    let start = region_start - hidden_before + prefix;
    // 一个隐藏跨度贡献 1 个投影行（而非 0）：投影行数 = 行数 - 隐藏行数 + 跨度数。
    let visible = region_end - region_start;
    let old_end = start + visible - hidden_lines(old_middle) + old_middle.len();
    let new_end = start + visible - hidden_lines(new_middle) + new_middle.len();
    Some(FoldEdit {
        old: ProjectedLineIndex::new(start)..ProjectedLineIndex::new(old_end),
        new: ProjectedLineIndex::new(start)..ProjectedLineIndex::new(new_end),
        changed_lines: Vec::new(),
        structural: true,
    })
}

fn hidden_lines(spans: &[Range<usize>]) -> usize {
    spans.iter().map(|span| span.end - span.start).sum()
}

/// 由 patch 的旧/新行区间派生**局部**结构编辑。
///
/// buffer 行经折叠跨度投影为 tab 行：折叠覆盖行投射到 anchor 行的合并行，
/// 其余行线性平移。因此编辑即使落在折叠内部，也只需重排它所在的合并行。
/// 批次要求整体重建时返回 None，调用方回退到整份重建。
fn linear_fold_edit(
    edits: &[FoldBufferEdit],
    old_buffer: &MultiBufferSnapshot,
    new_buffer: &MultiBufferSnapshot,
    old_spans: &[Range<usize>],
    new_spans: &[Range<usize>],
) -> Option<FoldEdit> {
    let mut old_rows: Option<Range<usize>> = None;
    let mut new_rows: Option<Range<usize>> = None;
    for edit in edits {
        let old_start = old_buffer.byte_to_line(edit.old.start).ok()?.get();
        let old_end = old_buffer.byte_to_line(edit.old.end).ok()?.get() + 1;
        let new_start = new_buffer.byte_to_line(edit.new.start).ok()?.get();
        let new_end = new_buffer.byte_to_line(edit.new.end).ok()?.get() + 1;
        old_rows = Some(merge_row_range(old_rows, old_start..old_end));
        new_rows = Some(merge_row_range(new_rows, new_start..new_end));
    }
    let old_spanned = projected_span(old_spans, &old_rows?);
    let new_spanned = projected_span(new_spans, &new_rows?);
    Some(FoldEdit {
        old: ProjectedLineIndex::new(old_spanned.start)..ProjectedLineIndex::new(old_spanned.end),
        new: ProjectedLineIndex::new(new_spanned.start)..ProjectedLineIndex::new(new_spanned.end),
        changed_lines: Vec::new(),
        structural: true,
    })
}

fn merge_row_range(existing: Option<Range<usize>>, next: Range<usize>) -> Range<usize> {
    match existing {
        Some(range) => range.start.min(next.start)..range.end.max(next.end),
        None => next,
    }
}

/// buffer 行区间的投影 tab 行区间：折叠覆盖的连续行坍缩为同一合并行。
fn projected_span(spans: &[Range<usize>], rows: &Range<usize>) -> Range<usize> {
    if rows.is_empty() {
        let row = projected_row(spans, rows.start);
        return row..row;
    }
    let first = projected_row(spans, rows.start);
    let last = projected_row(spans, rows.end - 1);
    first..last + 1
}

/// 单个 buffer 行投射到的 tab 行；折叠覆盖行投射到其 anchor 行的合并行。
fn projected_row(spans: &[Range<usize>], row: usize) -> usize {
    for span in spans {
        if row < span.start {
            break;
        }
        if row < span.end {
            let anchor = span.start - 1;
            return anchor - hidden_before(spans, anchor);
        }
    }
    row - hidden_before(spans, row)
}

/// 位于 `row` 之前的隐藏行总数（跨度的终点行必须严格早于 `row`）。
fn hidden_before(spans: &[Range<usize>], row: usize) -> usize {
    spans
        .iter()
        .filter(|span| span.end <= row)
        .map(|span| span.end - span.start)
        .sum()
}

fn inline_fold_edits(
    edits: &[FoldBufferEdit],
    inlay: &InlaySnapshot,
    folds: &SumTree<Fold>,
) -> Vec<FoldEdit> {
    edits
        .iter()
        .filter_map(|edit| {
            let buffer = inlay.buffer_snapshot();
            let start = buffer.byte_to_line(edit.new.start).ok()?;
            let end = buffer.byte_to_line(edit.new.end).ok()?;
            // changed_lines 是流行号（下游缓存失效按流行）。
            //
            // 折叠覆盖行（anchor 与 close 之间的隐藏行、close 行）的编辑映射到最外层折叠的 anchor 行：
            // 隐藏行编辑不改变显示文本；close 行编辑改变合并行尾段，合并行必须重排。
            // anchor 行自身可见，其编辑已在行列表中，不映射。
            let changed_lines = (start.get()..=end.get())
                .map(|line| {
                    let stream_line = Line::new(line);
                    folds
                        .iter()
                        .filter(|fold| {
                            let (span_start, span_end) = fold.line_span;
                            span_start.get() < line && line <= span_end.get()
                        })
                        .min_by_key(|fold| fold.line_span.0)
                        .map_or(stream_line, |fold| fold.line_span.0)
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

/// 折叠范围的逻辑行跨度：起点行（anchor）与终点所在行（close，含隐藏前缀）。
///
/// 折叠范围是字节级的（终点在 close 行内），终点行即被折叠的 close 行，不再按"终点恰在行首"回退（行首终点只可能来自旧的整行折叠形状）。
fn fold_line_span(
    snapshot: &MultiBufferSnapshot,
    range: MultiBufferRange,
) -> DisplayMapResult<(Line, Line)> {
    let start = snapshot.byte_to_line(range.start())?;
    let end = snapshot.byte_to_line(range.end())?;
    Ok((start, end))
}

fn ranges_disjoint_or_nested(left: MultiBufferRange, right: MultiBufferRange) -> bool {
    left.end() <= right.start()
        || right.end() <= left.start()
        || (left.start() <= right.start() && right.end() <= left.end())
        || (right.start() <= left.start() && left.end() <= right.end())
}

#[cfg(test)]
mod tests {
    use zcv_text::{Buffer, BufferConfig, Edit, TextSubscription, TransactionMetadata};

    use super::super::buffer_edits_from_batch;
    use super::super::error::DisplayMapError;
    use super::super::inlay_map::InlayMap;
    use super::*;

    fn text_range(start: usize, end: usize) -> MultiBufferRange {
        MultiBufferRange::new(MultiBufferOffset::new(start), MultiBufferOffset::new(end)).unwrap()
    }

    impl FoldMap {
        /// 测试辅助：按当前快照把字节范围折叠为组合锚点范围后写入。
        fn fold_text_range(
            &mut self,
            start: usize,
            end: usize,
        ) -> DisplayMapResult<(FoldSnapshot, Vec<FoldEdit>)> {
            let range = {
                let snapshot = self.snapshot.buffer_snapshot();
                snapshot.anchor_at(MultiBufferOffset::new(start), Affinity::Before)
                    ..snapshot.anchor_at(MultiBufferOffset::new(end), Affinity::After)
            };
            self.write().fold(range)
        }

        /// 测试辅助：把订阅者批次换算成 InlayEdit，再推进 fold 层。
        fn read_test(
            &mut self,
            buffer: &Buffer,
            subscription: &TextSubscription,
        ) -> (FoldSnapshot, Vec<FoldEdit>) {
            let new_snapshot: MultiBufferSnapshot = buffer.snapshot().into();
            let old_snapshot = self.snapshot.buffer_snapshot().clone();
            let batch = subscription.consume();
            let buffer_edits = buffer_edits_from_batch(&batch, &old_snapshot, &new_snapshot);
            let mut inlay_map = InlayMap::new(old_snapshot).0;
            let (inlay_snapshot, inlay_edits) =
                inlay_map.sync(new_snapshot, buffer_edits, Vec::new());
            self.read(inlay_snapshot, inlay_edits)
        }
    }

    #[test]
    fn projected_kind_rejects_the_end_boundary() {
        let buffer = Buffer::from_text("first\nsecond".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let (_, snapshot) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);

        assert!(
            snapshot
                .projected_kind(ProjectedLineIndex::new(snapshot.line_count()))
                .is_none()
        );
    }

    #[test]
    fn fold_snapshot_owns_fold_and_transform_trees_and_keeps_old_snapshots_stable() {
        let buffer = Buffer::from_text(
            "anchor\nhidden one\nhidden two\nafter".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let (mut map, before) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        let (after, edits) = map.fold_text_range(6, 21).unwrap();

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
    fn folding_a_middle_range_emits_a_localized_structural_edit() {
        let buffer = Buffer::from_text("a\nb\nc\nd\ne\nf\n".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        let (after, edits) = map.fold_text_range(2, 7).unwrap();

        assert_eq!(after.line_count(), 5);
        let edit = &edits[0];
        assert!(edit.is_structural());
        // 只覆盖被折叠的 tab 行，折叠点前后的可见行保留原变换。
        assert_eq!(edit.old_rows(), 2..4);
        assert_eq!(edit.new_rows(), 2..3);
    }

    #[test]
    fn unfolding_a_middle_fold_restores_only_its_rows() {
        let buffer = Buffer::from_text("a\nb\nc\nd\ne\nf\n".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        map.fold_text_range(2, 7).unwrap();
        let (after, edits) = map
            .write()
            .unfold_lines(LineRange::new(Line::new(0), Line::new(7)).unwrap())
            .unwrap();

        assert_eq!(after.line_count(), 7);
        let edit = &edits[0];
        assert!(edit.is_structural());
        assert_eq!(edit.old_rows(), 2..3);
        assert_eq!(edit.new_rows(), 2..4);
    }

    #[test]
    fn fold_writer_rejects_partial_overlap_but_accepts_nesting() {
        let buffer = Buffer::from_text("abcdef".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        map.fold_text_range(1, 5).unwrap();
        map.fold_text_range(2, 4).unwrap();

        let error = map.fold_text_range(0, 3).unwrap_err();
        assert!(matches!(
            error,
            DisplayMapError::Fold(FoldError::OverlapWithoutNesting { .. })
        ));
        assert_eq!(map.snapshot.folds.summary().count, 2);
    }

    #[test]
    fn unfolding_outer_fold_reveals_the_nested_transform() {
        let buffer =
            Buffer::from_text("a\nb\nc\nd\ne".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        map.fold_text_range(1, 7).unwrap();
        map.fold_text_range(3, 5).unwrap();
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
            Buffer::from_text("anchor\nhidden\nafter".to_string(), BufferConfig::default())
                .unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        map.fold_text_range(6, 13).unwrap();
        let transforms = map.snapshot.transforms.clone();
        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(MultiBufferOffset::new(9).into(), "!").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();

        let (snapshot, edits) = map.read_test(&buffer, &subscription);
        assert_eq!(snapshot.transforms, transforms);
        assert_eq!(snapshot.folds.summary().count, 1);
        assert!(edits.iter().all(|edit| !edit.is_structural()));
    }

    #[test]
    fn editing_inside_a_fold_remeasures_only_the_merged_row() {
        let mut buffer =
            Buffer::from_text("anchor\nhidden\nafter".to_string(), BufferConfig::default())
                .unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        map.fold_text_range(6, 13).unwrap();
        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(MultiBufferOffset::new(9).into(), "new\n").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();

        let (snapshot, edits) = map.read_test(&buffer, &subscription);
        // 折叠内部插入整行：隐藏行数随之变化，tab 行数不变；只有合并行需要重排。
        assert_eq!(snapshot.line_count(), 2);
        let edit = &edits[0];
        assert!(edit.is_structural());
        assert_eq!(edit.old_rows(), 0..1);
        assert_eq!(edit.new_rows(), 0..1);
    }

    #[test]
    fn newline_edit_rebuilds_transform_tree_and_emits_structural_fold_edit() {
        let mut buffer =
            Buffer::from_text("anchor\nhidden\nafter".to_string(), BufferConfig::default())
                .unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        map.fold_text_range(6, 13).unwrap();
        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(MultiBufferOffset::new(9).into(), "new\n").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();

        let (snapshot, edits) = map.read_test(&buffer, &subscription);
        assert_eq!(
            snapshot.logical_line_count(),
            buffer.snapshot().line_count()
        );
        assert!(edits.iter().all(FoldEdit::is_structural));
    }
    #[test]
    fn newline_edit_outside_folds_emits_a_localized_structural_edit() {
        let mut buffer =
            Buffer::from_text("a\nb\nc\nd\ne\nf\n".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        let subscription = buffer.subscribe();
        // 在未折叠区域插入换行：只应重排该行附近的 tab 行，而不是整份文档。
        buffer
            .edit(
                [Edit::insert(MultiBufferOffset::new(4).into(), "\n").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();

        let (snapshot, edits) = map.read_test(&buffer, &subscription);
        assert_eq!(snapshot.line_count(), 8);
        let edit = &edits[0];
        assert!(edit.is_structural());
        assert_eq!(edit.old_rows(), 2..3);
        assert_eq!(edit.new_rows(), 2..4);
    }

    #[test]
    fn deleting_folded_text_invalidates_anchor_range() {
        let mut buffer =
            Buffer::from_text("anchor\nhidden\nafter".to_string(), BufferConfig::default())
                .unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        map.fold_text_range(6, 13).unwrap();
        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::delete(text_range(0, "anchor\nhidden\n".len()).into())],
                TransactionMetadata::default(),
            )
            .unwrap();

        let (snapshot, _) = map.read_test(&buffer, &subscription);
        assert_eq!(snapshot.folds.summary().count, 0);
        assert_eq!(
            snapshot.line_count(),
            snapshot.buffer_snapshot().line_count()
        );
    }

    #[test]
    fn merged_row_text_joins_anchor_placeholder_and_close_tail() {
        // 折叠范围 = [anchor 行换行符, 闭合括号前)：anchor 文本、占位符、真实 `}` 拼成同一行。
        let buffer = Buffer::from_text(
            "fn b() {\n    2\n}\nrest".to_string(),
            BufferConfig::default(),
        )
        .unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        // range = [行 0 换行符(8), `}`(15))。
        let (snapshot, _) = map.fold_text_range(8, 15).unwrap();

        assert_eq!(snapshot.line_count(), 2);
        let text = snapshot.row_text(ProjectedLineIndex::new(0)).unwrap();
        assert_eq!(text.as_ref(), "fn b() {…}\n");
        // 段表：anchor 文本段 + 占位符段 + 闭合尾段（`}` 是真实字节范围）。
        let segments = snapshot
            .fold_row_segments(ProjectedLineIndex::new(0))
            .unwrap();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].merged_range, 0..8);
        assert_eq!(segments[1].merged_range, 8..11);
        assert_eq!(segments[2].merged_range, 11..12);
    }

    #[test]
    fn fold_boundary_insertions_remain_visible() {
        // Stickiness::Never：折叠起点插入的文本在折叠外（可见），折叠终点插入的文本在折叠内。
        let mut buffer =
            Buffer::from_text("anchor\nhidden\nafter".to_string(), BufferConfig::default())
                .unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        map.fold_text_range(6, 13).unwrap();
        let fold_range = map.snapshot.folds.iter().next().unwrap().text_range();
        // 折叠起点 = anchor 行换行符位置（6）。
        assert_eq!(fold_range.start().get(), 6);
        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(fold_range.start().into(), "X").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
        let (snapshot, _) = map.read_test(&buffer, &subscription);
        // 起点插入在折叠外：折叠范围随插入右移，anchor 行文本变为 "anchorX"。
        let moved = snapshot.folds.iter().next().unwrap().text_range();
        assert_eq!(moved.start().get(), 7);
        let text = snapshot.row_text(ProjectedLineIndex::new(0)).unwrap();
        assert_eq!(text.as_ref(), "anchorX…\n");
    }

    #[test]
    fn edits_on_folded_lines_map_to_anchor_row() {
        let mut buffer =
            Buffer::from_text("anchor\nhidden\nafter".to_string(), BufferConfig::default())
                .unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        map.fold_text_range(6, 13).unwrap();
        // 编辑落在隐藏行（行 1）与 close 行（行 2）：changed_lines 都映射到 anchor 行（行 0）。
        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(MultiBufferOffset::new(9).into(), "X").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
        let (_, edits) = map.read_test(&buffer, &subscription);
        assert_eq!(edits[0].changed_lines(), &[Line::ZERO]);
    }

    #[test]
    fn folded_points_map_through_anchor_in_both_directions() {
        let buffer = Buffer::from_text("a\nb\nc\nd".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(InlayMap::new(buffer.snapshot().into()).1);
        let (snapshot, _) = map.fold_text_range(1, 5).unwrap();

        // 折叠段不产生投影行：行数 = 4 - 2 隐藏 = 2。
        assert_eq!(snapshot.line_count(), 2);
        // 隐藏行按 bias 吸附到合并行（anchor 行 0）的折叠起点/终点列；
        // anchor 行 "a" 内容 1 字符，占位符 1 字符。
        let left = snapshot
            .logical_to_projected_point(
                LogicalPoint::new(Line::new(1), LogicalColumn::ZERO),
                FoldBias::Left,
            )
            .unwrap();
        assert_eq!(
            left,
            ProjectedPoint::new(ProjectedLineIndex::new(0), LogicalColumn::new(1))
        );
        let right = snapshot
            .logical_to_projected_point(
                LogicalPoint::new(Line::new(1), LogicalColumn::ZERO),
                FoldBias::Right,
            )
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
pub(crate) struct ProjectedLineIndex(usize);

impl ProjectedLineIndex {
    pub(crate) const ZERO: Self = Self(0);

    pub(crate) const fn new(value: usize) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> usize {
        self.0
    }
}

/// 可见逻辑行投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TextLine {
    logical_line: Line,
}

impl TextLine {
    pub(crate) fn new(logical_line: Line) -> Self {
        Self { logical_line }
    }

    pub(crate) fn logical_line(self) -> Line {
        self.logical_line
    }
}

/// 逻辑行 -> 投影空间的查询结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum LogicalProjection {
    /// 逻辑行可见，对应投影行索引。
    Visible(ProjectedLineIndex),
    /// 逻辑行被某段 fold 隐藏。
    Hidden,
}

/// 逻辑文档内的 (line, column) 点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(super) struct LogicalPoint {
    line: Line,
    column: LogicalColumn,
}

impl LogicalPoint {
    pub(super) const fn new(line: Line, column: LogicalColumn) -> Self {
        Self { line, column }
    }

    pub(super) const fn line(self) -> Line {
        self.line
    }

    pub(super) const fn column(self) -> LogicalColumn {
        self.column
    }

    pub(super) fn into_position(self) -> Position {
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
pub(crate) struct ProjectedPoint {
    line: ProjectedLineIndex,
    column: LogicalColumn,
}

impl ProjectedPoint {
    pub(crate) const fn new(line: ProjectedLineIndex, column: LogicalColumn) -> Self {
        Self { line, column }
    }

    pub(crate) const fn line(self) -> ProjectedLineIndex {
        self.line
    }

    pub(crate) const fn column(self) -> LogicalColumn {
        self.column
    }
}

/// 折叠合并行文本的段：合并文本字节空间的切分（只服务列↔字节映射与高亮坐标域，不参与行数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoldRowSegment {
    /// 段在合并文本中的投影字节范围。
    pub(super) merged_range: Range<usize>,
    pub(super) kind: FoldRowSegmentKind,
}

impl FoldRowSegment {
    pub(crate) fn merged_range(&self) -> &Range<usize> {
        &self.merged_range
    }
}

/// 折叠合并行段的来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FoldRowSegmentKind {
    /// 行内容段：流行号与行内投影字节范围。
    Text {
        stream_line: Line,
        projected_range: Range<usize>,
    },
    /// 折叠占位符段（无源坐标）。
    Placeholder,
}

/// 折叠合并行（anchor 行文本 + 占位符 + 闭合行尾段）的投影几何。
#[derive(Debug, Clone, Copy)]
struct FoldMergedGeometry {
    /// 合并行的投影行号（anchor 行）。
    row: ProjectedLineIndex,
    /// anchor 行流行号。
    anchor_stream: Line,
    /// anchor 段字符数（含行内提示注入，不含行尾换行）。
    anchor_chars: usize,
    /// anchor 段投影字节数（合并文本内前缀长度）。
    anchor_len: usize,
    /// 闭合行（折叠范围终点所在行）。
    close_line: Line,
    /// 闭合行流行号。
    close_stream: Line,
    /// 尾段起点：闭合行内投影偏移（含行内提示注入）。
    tail_projected: usize,
    /// 尾段终点：闭合行内投影偏移（行内容末尾）。
    content_end_projected: usize,
    /// 尾段起点：闭合行原始列（隐藏前缀的字符数）。
    tail_start_col: usize,
}

/// 逻辑文档内的有序点对范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LogicalRange {
    start: LogicalPoint,
    end: LogicalPoint,
}

impl LogicalRange {
    /// 要求 `start <= end`（按 line, column 字典序）。
    pub(super) fn new(start: LogicalPoint, end: LogicalPoint) -> Result<Self, CoordinateError> {
        if !is_ordered_logical(start, end) {
            return Err(CoordinateError::InvalidLineRange {
                start: start.line,
                end: end.line,
            });
        }
        Ok(Self { start, end })
    }

    pub(super) const fn start(self) -> LogicalPoint {
        self.start
    }

    pub(super) const fn end(self) -> LogicalPoint {
        self.end
    }

    pub(super) fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// 投影空间内的有序点对范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProjectedRange {
    start: ProjectedPoint,
    end: ProjectedPoint,
}

impl ProjectedRange {
    /// 要求 `start <= end`（按 projected line, column 字典序）。
    pub(crate) fn new(start: ProjectedPoint, end: ProjectedPoint) -> Result<Self, CoordinateError> {
        if !is_ordered_projected(start, end) {
            return Err(CoordinateError::InvalidLineRange {
                start: Line::new(start.line.get()),
                end: Line::new(end.line.get()),
            });
        }
        Ok(Self { start, end })
    }

    pub(crate) const fn start(self) -> ProjectedPoint {
        self.start
    }

    pub(crate) const fn end(self) -> ProjectedPoint {
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
