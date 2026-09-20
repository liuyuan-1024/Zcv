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

/// 一段组合文本坐标的编辑，供 fold 拓扑定位使用。
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
    /// 下层组合文本快照：fold 拓扑工作在其上，外部文本可被折叠。
    input: MultiBufferSnapshot,
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
    pub(super) fn buffer_snapshot(&self) -> &MultiBufferSnapshot {
        &self.input
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
        let buffer = self.buffer_snapshot();
        let anchor_stream = anchor;
        let (anchor_len, anchor_chars) = buffer
            .line_content_metrics(anchor_stream)
            .expect("折叠 anchor 行必须位于流内");
        let close_line = fold.line_span.1;
        let close_stream = close_line;
        let close_start = buffer
            .line_start_byte(close_line)
            .expect("折叠 close 行必须位于当前 Snapshot 内");
        let tail_projected = fold.text_range().end().get() - close_start.get();
        let content_end_projected = buffer
            .line_content_metrics(close_stream)
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
        self.lookup.anchor_by_line.get(&text.logical_line())
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
        Ok(byte - close_start.get() - geometry.tail_projected)
    }

    /// 投影行的内容来源：fold 投影（Text）叠加流行解析。
    ///
    /// 文本行统一携带对应的 buffer 行来源。
    pub(super) fn projected_kind(
        &self,
        projected: ProjectedLineIndex,
    ) -> Option<StreamProjectedKind> {
        let text = self.projected_line_kind(projected)?;
        Some(StreamProjectedKind::Text(text.logical_line()))
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
    pub(super) fn new(input: MultiBufferSnapshot) -> (Self, FoldSnapshot) {
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
        input: MultiBufferSnapshot,
        buffer_edits: Vec<FoldBufferEdit>,
    ) -> (FoldSnapshot, Vec<FoldEdit>) {
        let old_buffer = self.snapshot.buffer_snapshot().clone();
        if input.version() == old_buffer.version() {
            // 文本未变，但捕获表或元数据可能已更新：
            // 采用新输入快照，保证 fold 链上仍持有当前 MultiBufferSnapshot。
            self.snapshot.input = input;
            return (self.snapshot.clone(), Vec::new());
        }

        let buffer = input.clone();
        let old_spans = hidden_spans(&self.snapshot.folds);
        let mut retained = Vec::new();
        self.snapshot.fold_metadata_by_id.clear();
        // 活动折叠只保存组合锚点：编辑后按新快照重新解析，不再手工重映射裸偏移。
        for fold in self.snapshot.folds.iter() {
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
        self.snapshot.version += 1;
        // 结构编辑的投影区间由旧/新变换树直接映射；旧树必须先冻结再替换。
        let old_transforms = structural.then(|| {
            let spans = hidden_spans(&self.snapshot.folds);
            let next = build_transforms(&spans, buffer.line_count());
            std::mem::replace(&mut self.snapshot.transforms, next)
        });
        self.snapshot.input = input;
        let edits = if let Some(old_transforms) = old_transforms {
            vec![linear_fold_edit(
                &buffer_edits,
                &old_transforms,
                &self.snapshot.transforms,
                &old_buffer,
                &buffer,
            )]
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
        let range = resolved.start()..resolved.end();
        let mut folds: Vec<_> = self.0.snapshot.folds.iter().cloned().collect();
        let fold = Fold::from_text_range(self.0.snapshot.buffer_snapshot(), id, resolved)
            .ok_or(FoldError::UnresolvableAnchor)?;
        folds.push(fold);
        sort_folds(&mut folds);
        self.0.snapshot.folds = SumTree::from_iter(folds, ());
        let indexed_folds = self.0.snapshot.folds.iter().cloned().collect::<Vec<_>>();
        self.0.snapshot.lookup = FoldLookup::from_folds(&indexed_folds);
        self.0.snapshot.fold_metadata_by_id.insert(id, resolved);
        // 先推进变换树，再克隆对外快照；
        // 否则返回值仍是旧 transforms，折叠不隐藏任何行。
        let edits = self.fold_topology_edits(range, stream_line_count);
        Ok((self.0.snapshot.clone(), edits))
    }

    /// 折叠拓扑变更的本层编辑：与文本编辑共用同一套变换树推导。
    ///
    /// 输入区间取折叠刚变更的字节范围（文本未变，old == new），由旧/新变换树映射出本层失效区间，与 Zed 的 FoldMap::sync 同一契约。
    fn fold_topology_edits(
        &mut self,
        range: Range<MultiBufferOffset>,
        stream_line_count: usize,
    ) -> Vec<FoldEdit> {
        let spans = hidden_spans(&self.0.snapshot.folds);
        let next_transforms = build_transforms(&spans, stream_line_count);
        let old_transforms = std::mem::replace(&mut self.0.snapshot.transforms, next_transforms);
        self.0.snapshot.version += 1;
        let buffer = self.0.snapshot.buffer_snapshot().clone();
        let edit = ProjectionEdit::new(range.clone(), range);
        vec![linear_fold_edit(
            std::slice::from_ref(&edit),
            &old_transforms,
            &self.0.snapshot.transforms,
            &buffer,
            &buffer,
        )]
    }

    fn unfold(&mut self, id: FoldId) -> (FoldSnapshot, Vec<FoldEdit>) {
        if !self.0.snapshot.fold_metadata_by_id.contains_key(&id) {
            return (self.0.snapshot.clone(), Vec::new());
        }
        let stream_line_count = self.0.snapshot.input.line_count();
        let range = self
            .0
            .snapshot
            .folds
            .iter()
            .find(|fold| fold.id == id)
            .map(|fold| fold.text_range())
            .map(|range| range.start()..range.end())
            .expect("已确认折叠存在于当前快照");
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
        // 先推进变换树，再克隆对外快照；展开与折叠共用同一顺序约束。
        let edits = self.fold_topology_edits(range, stream_line_count);
        (self.0.snapshot.clone(), edits)
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

/// 由 patch 的旧/新行区间派生**局部**结构编辑。
///
/// 输入行区间经旧/新变换树映射到投影行；端点落在折叠段内时吸附到整个折叠段。
/// 吸附保证编辑覆盖完整的折叠变换，投影行数守恒由变换树本身保证，
/// 上层因此不需要任何整层回退。
/// 编辑区间必须能在对应快照上解析为行；不可解析说明 patch 与快照不一致，直接失败。
fn linear_fold_edit(
    edits: &[FoldBufferEdit],
    old_transforms: &SumTree<Transform>,
    new_transforms: &SumTree<Transform>,
    old_buffer: &MultiBufferSnapshot,
    new_buffer: &MultiBufferSnapshot,
) -> FoldEdit {
    let mut old_rows: Option<Range<usize>> = None;
    let mut new_rows: Option<Range<usize>> = None;
    for edit in edits {
        let old_start = old_buffer
            .byte_to_line(edit.old.start)
            .expect("结构编辑的旧区间起点必须落在旧快照内")
            .get();
        let old_end = old_buffer
            .byte_to_line(edit.old.end)
            .expect("结构编辑的旧区间终点必须落在旧快照内")
            .get()
            + 1;
        let new_start = new_buffer
            .byte_to_line(edit.new.start)
            .expect("结构编辑的新区间起点必须落在新快照内")
            .get();
        let new_end = new_buffer
            .byte_to_line(edit.new.end)
            .expect("结构编辑的新区间终点必须落在新快照内")
            .get()
            + 1;
        old_rows = Some(merge_row_range(old_rows, old_start..old_end));
        new_rows = Some(merge_row_range(new_rows, new_start..new_end));
    }
    let old_rows = old_rows.expect("结构编辑必须至少包含一段文本编辑");
    let new_rows = new_rows.expect("结构编辑必须至少包含一段文本编辑");
    let old_spanned = projected_rows_for_input_range(old_transforms, &old_rows);
    let new_spanned = projected_rows_for_input_range(new_transforms, &new_rows);
    // 编辑完全落在折叠段内时投影区间为空，但合并行（anchor 行）承载了变化后的占位/尾段文本，必须把该行一并失效，否则合并行宽度缓存不会重排。
    let old_spanned = expand_fold_interior(old_spanned, &old_rows);
    let new_spanned = expand_fold_interior(new_spanned, &new_rows);
    // 行级失效区间无法像 Zed 的偏移编辑那样表达行内变化：必须让本层区间覆盖两侧权威净行数，
    // 否则 Wrap 变换树的输入行数会与 tab 快照失配。这里只做有界补齐，不是整层失效。
    let global_delta = new_transforms.summary().output_rows as isize
        - old_transforms.summary().output_rows as isize;
    let target_new_len = (old_spanned.len() as isize + global_delta).max(0) as usize;
    let new_end = new_spanned.start + target_new_len;
    FoldEdit {
        old: ProjectedLineIndex::new(old_spanned.start)..ProjectedLineIndex::new(old_spanned.end),
        new: ProjectedLineIndex::new(new_spanned.start)..ProjectedLineIndex::new(new_end),
        changed_lines: Vec::new(),
        structural: true,
    }
}

/// 折叠段内部的编辑不产生投影行；把空投影区间吸附到其前方的合并行。
fn expand_fold_interior(projected: Range<usize>, input: &Range<usize>) -> Range<usize> {
    if projected.is_empty() && !input.is_empty() && projected.start > 0 {
        projected.start - 1..projected.start
    } else {
        projected
    }
}

fn merge_row_range(existing: Option<Range<usize>>, next: Range<usize>) -> Range<usize> {
    match existing {
        Some(range) => range.start.min(next.start)..range.end.max(next.end),
        None => next,
    }
}

/// 把输入（流行）行区间映射到投影行区间，端点落在折叠段内时吸附到整个折叠段。
///
/// 折叠段不产生投影行，区间端点若落在其中且不吸附，投影编辑会漏掉或错位一行。
/// 吸附后区间两端都对齐到完整变换边界，投影行数与之守恒。
fn projected_rows_for_input_range(
    transforms: &SumTree<Transform>,
    rows: &Range<usize>,
) -> Range<usize> {
    let mut cursor = transforms.cursor::<InputToOutput>(());
    cursor.seek(&InputLines(rows.start), TreeBias::Left);
    let mut start = rows.start;
    if cursor
        .item()
        .is_some_and(|transform| transform.kind == TransformKind::Fold)
    {
        start = cursor.start().0.0;
    }
    let output_start = cursor.start().1.0 + (start - cursor.start().0.0);
    if rows.is_empty() {
        // 空区间没有内容变化：映射为空输出区间，吸附不得使其扩张。
        return output_start..output_start;
    }

    cursor.seek_forward(&InputLines(rows.end), TreeBias::Right);
    let mut end = rows.end;
    if cursor
        .item()
        .is_some_and(|transform| transform.kind == TransformKind::Fold)
    {
        cursor.next();
        end = cursor.start().0.0;
    }
    let output_end = cursor.start().1.0 + (end - cursor.start().0.0);

    output_start..output_end
}

fn inline_fold_edits(
    edits: &[FoldBufferEdit],
    buffer: &MultiBufferSnapshot,
    folds: &SumTree<Fold>,
) -> Vec<FoldEdit> {
    edits
        .iter()
        .filter_map(|edit| {
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
#[path = "test/fold_map_tests.rs"]
mod tests;

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
