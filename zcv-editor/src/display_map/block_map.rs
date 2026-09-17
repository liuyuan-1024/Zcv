//! 多文件 Editor 的块级显示投影。
//!
//! 本层位于 WrapMap 之上：文本换行坐标保持不变，文件标题和同文件片段分隔线作为不属于文本的虚拟显示块插入。
//! 这样搜索、diff、诊断等宿主只负责提供 excerpts，滚动、命中测试、选区和通用文件标题都由 Editor 复用同一条管线。

use std::collections::{BTreeSet, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use sum_tree::{Bias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use zcv_multi_buffer::ExcerptSnapshot;
use zcv_text::{ByteOffset, CoordinateError, Line, TextRange};

use super::error::DisplayMapResult;
use super::fold_map::{FoldBias, ProjectedLineIndex, ProjectedPoint, ProjectedRange};
use super::wrap_map::{WrapRowKind, WrapRows, WrapSnapshot};
use super::{DisplayPoint, DisplayRow};

pub(crate) const FILE_HEADER_HEIGHT: usize = 2;
pub(super) const EXCERPT_BOUNDARY_HEIGHT: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayBlockKind {
    BufferHeader,
    ExcerptBoundary,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DisplayBlock {
    pub(crate) kind: DisplayBlockKind,
    pub(crate) excerpt: ExcerptSnapshot,
}

/// 由当前滚动位置派生的悬浮文件标题。
///
/// `source_row` 标识它对应的真实边界块；`next_buffer_header_row` 用于在下一个文件到达时把当前标题向上顶出。
/// 该结构只是一帧投影，不保存当前文件状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StickyBufferHeader {
    pub(crate) source_row: DisplayRow,
    pub(crate) excerpt: ExcerptSnapshot,
    pub(crate) next_buffer_header_row: Option<DisplayRow>,
}

#[derive(Clone, Debug)]
struct BlockPlacement {
    display_row: usize,
    height: usize,
    block: DisplayBlock,
    next_buffer_header_row: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransformKind {
    Text,
    Block(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Transform {
    kind: TransformKind,
    input_rows: usize,
    output_rows: usize,
}

impl Item for Transform {
    type Summary = TransformSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        TransformSummary {
            input_rows: self.input_rows,
            output_rows: self.output_rows,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TransformSummary {
    input_rows: usize,
    output_rows: usize,
}

impl ContextLessSummary for TransformSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.input_rows += summary.input_rows;
        self.output_rows += summary.output_rows;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct InputRows(usize);

impl<'a> Dimension<'a, TransformSummary> for InputRows {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.0 += summary.input_rows;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct OutputRows(usize);

impl<'a> Dimension<'a, TransformSummary> for OutputRows {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.0 += summary.output_rows;
    }
}

type InputToOutput = Dimensions<InputRows, OutputRows>;
type OutputToInput = Dimensions<OutputRows, InputRows>;

#[derive(Debug, Clone)]
pub(super) struct BlockSnapshot {
    wrap_snapshot: WrapSnapshot,
    transforms: SumTree<Transform>,
    placements: Vec<BlockPlacement>,
    excerpts: Arc<[ExcerptSnapshot]>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BlockRow {
    index: DisplayRow,
    height: usize,
    kind: BlockRowKind,
    excerpt: Option<ExcerptSnapshot>,
}

impl BlockRow {
    pub(crate) fn index(&self) -> DisplayRow {
        self.index
    }

    pub(crate) fn height(&self) -> usize {
        self.height
    }

    pub(crate) fn kind(&self) -> &WrapRowKind {
        match &self.kind {
            BlockRowKind::Text(kind) => kind,
            BlockRowKind::Block(_) => {
                panic!("虚拟块没有文本行类型；调用方应先查询 block()")
            }
        }
    }

    pub(crate) fn block(&self) -> Option<&DisplayBlock> {
        match &self.kind {
            BlockRowKind::Text(_) => None,
            BlockRowKind::Block(block) => Some(block),
        }
    }

    pub(crate) fn excerpt(&self) -> Option<&ExcerptSnapshot> {
        self.excerpt.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq)]
enum BlockRowKind {
    Text(WrapRowKind),
    Block(DisplayBlock),
}

/// Block/Wrap 连续显示流的游标。
///
/// 它是布局和命中测试共同使用的显示快照入口：Block 层只负责插入虚拟块，
/// 文本行交给同一个 `WrapRows` 游标连续消费。虚拟块不再迫使调用方重新查询整段行映射。
pub(crate) struct BlockRows<'a> {
    snapshot: &'a BlockSnapshot,
    block_cursor: sum_tree::Cursor<'a, 'static, Transform, OutputToInput>,
    wrap_rows: WrapRows<'a>,
    row: usize,
    end: usize,
    excerpt_index: Option<usize>,
}

impl<'a> BlockRows<'a> {
    pub(super) fn new(
        snapshot: &'a BlockSnapshot,
        start_row: DisplayRow,
        line_count: usize,
    ) -> Self {
        let start = start_row.get().min(snapshot.line_count());
        let end = start.saturating_add(line_count).min(snapshot.line_count());
        let mut block_cursor = snapshot.transforms.cursor::<OutputToInput>(());
        block_cursor.seek(&OutputRows(start), Bias::Right);
        let wrap_rows = snapshot
            .wrap_snapshot
            .rows(0, snapshot.wrap_snapshot.line_count());
        let mut this = Self {
            snapshot,
            block_cursor,
            wrap_rows,
            row: start,
            end,
            excerpt_index: None,
        };
        this.seek_wrap_to_current_transform();
        this
    }

    fn seek_wrap_to_current_transform(&mut self) {
        if self.block_cursor.item().is_some() {
            let start = *self.block_cursor.start();
            self.wrap_rows.seek_forward(start.1.0);
        }
    }

    pub(crate) fn next(&mut self) -> Option<BlockRow> {
        if self.row >= self.end {
            return None;
        }
        let transform = self.block_cursor.item()?;
        let transform_start = *self.block_cursor.start();
        match transform.kind {
            TransformKind::Block(placement_index) => {
                let placement = &self.snapshot.placements[placement_index];
                self.row = (placement.display_row + placement.height).min(self.end);
                self.block_cursor.next();
                self.seek_wrap_to_current_transform();
                Some(BlockRow {
                    index: DisplayRow::new(placement.display_row),
                    height: placement.height,
                    kind: BlockRowKind::Block(placement.block.clone()),
                    excerpt: None,
                })
            }
            TransformKind::Text => {
                let wrap_row = transform_start.1.0 + self.row - transform_start.0.0;
                self.wrap_rows.seek_forward(wrap_row);
                let wrap = self.wrap_rows.next()?;
                let excerpt = match &wrap {
                    WrapRowKind::Text { source, .. } => self
                        .snapshot
                        .excerpt_for_line(source.line(), &mut self.excerpt_index)
                        .cloned(),
                };
                let row = BlockRow {
                    index: DisplayRow::new(self.row),
                    height: 1,
                    kind: BlockRowKind::Text(wrap),
                    excerpt,
                };
                self.row += 1;
                if self.row >= transform_start.0.0 + transform.output_rows {
                    self.block_cursor.next();
                    self.seek_wrap_to_current_transform();
                }
                Some(row)
            }
        }
    }

    pub(crate) fn source_line_ranges(mut self) -> Vec<Range<Line>> {
        let stream = self
            .snapshot
            .wrap_snapshot
            .tab_snapshot()
            .fold_snapshot()
            .inlay_snapshot()
            .stream();
        let mut lines = BTreeSet::new();
        while let Some(row) = self.next() {
            let BlockRowKind::Text(WrapRowKind::Text {
                source,
                projected_line,
                ..
            }) = row.kind
            else {
                continue;
            };
            if let Some(segments) = self
                .snapshot
                .wrap_snapshot
                .tab_snapshot()
                .fold_snapshot()
                .fold_row_segments(ProjectedLineIndex::new(projected_line))
            {
                for segment in segments.iter() {
                    if let super::fold_map::FoldRowSegmentKind::Text { stream_line, .. } =
                        segment.kind
                        && let Some(source) = stream.source(stream_line)
                    {
                        lines.insert(source.line());
                    }
                }
            } else {
                lines.insert(source.line());
            }
        }
        let mut ranges = Vec::new();
        let mut lines = lines.into_iter().peekable();
        while let Some(start) = lines.next() {
            let mut end = start;
            while lines.peek().is_some_and(|line| *line == end + 1) {
                end = lines.next().expect("连续源行的下一个元素必须存在");
            }
            ranges.push(Line::new(start)..Line::new(end.saturating_add(1)));
        }
        ranges
    }
}

enum RowMapping<'a> {
    Text(DisplayRow),
    Block(&'a BlockPlacement),
}

impl BlockSnapshot {
    pub(super) fn wrap_snapshot(&self) -> &WrapSnapshot {
        &self.wrap_snapshot
    }

    pub(crate) fn rows(&self, start_row: DisplayRow, line_count: usize) -> BlockRows<'_> {
        BlockRows::new(self, start_row, line_count)
    }

    pub(super) fn new(
        wrap_snapshot: WrapSnapshot,
        excerpts: &[ExcerptSnapshot],
        folded_buffers: &HashSet<PathBuf>,
    ) -> Self {
        struct BlockSpec {
            wrap_row: usize,
            height: usize,
            block: DisplayBlock,
            hide_until: Option<usize>,
        }

        let excerpt_starts = excerpts
            .iter()
            .filter(|excerpt| excerpt.starts_new_excerpt())
            .filter_map(|excerpt| {
                wrap_snapshot
                    .offset_to_display_point(excerpt.output_range().start())
                    .ok()
                    .map(|point| (point.row().get(), excerpt.clone()))
            })
            .collect::<Vec<_>>();
        let mut specs = Vec::new();
        let mut group_start = 0usize;
        while group_start < excerpt_starts.len() {
            let path = excerpt_starts[group_start].1.path();
            let mut group_end = group_start + 1;
            while group_end < excerpt_starts.len() && excerpt_starts[group_end].1.path() == path {
                group_end += 1;
            }
            let wrap_end = excerpt_starts
                .get(group_end)
                .map_or_else(|| wrap_snapshot.line_count(), |(row, _)| *row);
            if folded_buffers.contains(path) {
                let (wrap_row, excerpt) = &excerpt_starts[group_start];
                specs.push(BlockSpec {
                    wrap_row: *wrap_row,
                    height: FILE_HEADER_HEIGHT,
                    block: DisplayBlock {
                        kind: DisplayBlockKind::BufferHeader,
                        excerpt: excerpt.clone(),
                    },
                    hide_until: Some(wrap_end),
                });
            } else {
                for (index, (wrap_row, excerpt)) in
                    excerpt_starts[group_start..group_end].iter().enumerate()
                {
                    let kind = if index == 0 {
                        DisplayBlockKind::BufferHeader
                    } else {
                        DisplayBlockKind::ExcerptBoundary
                    };
                    specs.push(BlockSpec {
                        wrap_row: *wrap_row,
                        height: match kind {
                            DisplayBlockKind::BufferHeader => FILE_HEADER_HEIGHT,
                            DisplayBlockKind::ExcerptBoundary => EXCERPT_BOUNDARY_HEIGHT,
                        },
                        block: DisplayBlock {
                            kind,
                            excerpt: excerpt.clone(),
                        },
                        hide_until: None,
                    });
                }
            }
            group_start = group_end;
        }

        specs.sort_by_key(|spec| spec.wrap_row);

        let wrap_line_count = wrap_snapshot.line_count();
        let mut placements = Vec::new();
        let mut transforms = Vec::new();
        let mut wrap_row = 0usize;
        let mut display_row = 0usize;
        for spec in specs {
            let spec_wrap_row = spec.wrap_row.min(wrap_line_count);
            if wrap_row < spec_wrap_row {
                let row_count = spec_wrap_row - wrap_row;
                transforms.push(Transform {
                    kind: TransformKind::Text,
                    input_rows: row_count,
                    output_rows: row_count,
                });
                wrap_row = spec_wrap_row;
                display_row += row_count;
            }
            let placement_index = placements.len();
            placements.push(BlockPlacement {
                display_row,
                height: spec.height,
                block: spec.block,
                next_buffer_header_row: None,
            });
            let hidden_end = spec
                .hide_until
                .map_or(wrap_row, |end| end.min(wrap_line_count));
            transforms.push(Transform {
                kind: TransformKind::Block(placement_index),
                input_rows: hidden_end.saturating_sub(wrap_row),
                output_rows: spec.height,
            });
            wrap_row = hidden_end;
            display_row += spec.height;
        }
        if wrap_row < wrap_line_count {
            let row_count = wrap_line_count - wrap_row;
            transforms.push(Transform {
                kind: TransformKind::Text,
                input_rows: row_count,
                output_rows: row_count,
            });
        }

        let mut next_buffer_header_row = None;
        for placement in placements.iter_mut().rev() {
            placement.next_buffer_header_row = next_buffer_header_row;
            if placement.block.kind == DisplayBlockKind::BufferHeader {
                next_buffer_header_row = Some(placement.display_row);
            }
        }

        Self {
            wrap_snapshot,
            transforms: SumTree::from_iter(transforms, ()),
            placements,
            excerpts: Arc::from(excerpts),
        }
    }

    pub(super) fn line_count(&self) -> usize {
        self.transforms.summary().output_rows
    }

    /// 返回视口顶部所在 excerpt 的文件标题，以及下一个文件标题的位置。
    ///
    /// 同一文件的后续 excerpt 只有分隔块，但它同样会更新标题所代表的 excerpt，使“打开文件”等操作仍以当前可见片段为目标。
    pub(super) fn sticky_buffer_header(&self, top_row: DisplayRow) -> Option<StickyBufferHeader> {
        let index = self
            .placements
            .partition_point(|placement| placement.display_row <= top_row.get())
            .checked_sub(1)?;
        let placement = &self.placements[index];
        Some(StickyBufferHeader {
            source_row: DisplayRow::new(placement.display_row),
            excerpt: placement.block.excerpt.clone(),
            next_buffer_header_row: placement.next_buffer_header_row.map(DisplayRow::new),
        })
    }

    fn wrap_row_to_display_row(&self, wrap_row: usize) -> usize {
        if wrap_row >= self.transforms.summary().input_rows {
            return self.line_count().saturating_sub(1);
        }
        let (start, _, transform) =
            self.transforms
                .find::<InputToOutput, _>((), &InputRows(wrap_row), Bias::Right);
        match transform.map(|transform| transform.kind) {
            Some(TransformKind::Text) => start.1.0 + wrap_row - start.0.0,
            Some(TransformKind::Block(placement)) => self.placements[placement].display_row,
            None => self.line_count().saturating_sub(1),
        }
    }

    pub(super) fn display_row_to_wrap_row(&self, display_row: DisplayRow) -> Option<DisplayRow> {
        if display_row.get() >= self.line_count() {
            return None;
        }
        match self.display_row_mapping(display_row.get()) {
            RowMapping::Text(row) => Some(row),
            RowMapping::Block(_) => None,
        }
    }

    pub(super) fn projected_wrap_row_to_display_row(&self, wrap_row: usize) -> DisplayRow {
        DisplayRow::new(self.wrap_row_to_display_row(wrap_row))
    }

    fn display_row_mapping(&self, display_row: usize) -> RowMapping<'_> {
        let (start, _, transform) =
            self.transforms
                .find::<OutputToInput, _>((), &OutputRows(display_row), Bias::Right);
        match transform.map(|transform| transform.kind) {
            Some(TransformKind::Text) => RowMapping::Text(DisplayRow::new(
                start.1.0 + display_row.saturating_sub(start.0.0),
            )),
            Some(TransformKind::Block(placement)) => RowMapping::Block(&self.placements[placement]),
            None => RowMapping::Text(DisplayRow::ZERO),
        }
    }

    pub(super) fn offset_to_display_point(
        &self,
        offset: ByteOffset,
    ) -> DisplayMapResult<DisplayPoint> {
        let point = self.wrap_snapshot.offset_to_display_point(offset)?;
        Ok(DisplayPoint::new(
            DisplayRow::new(self.wrap_row_to_display_row(point.row().get())),
            point.column(),
        ))
    }

    pub(super) fn display_point_to_offset_with_bias(
        &self,
        point: DisplayPoint,
        bias: FoldBias,
    ) -> DisplayMapResult<ByteOffset> {
        if point.row().get() >= self.line_count() {
            return Err(CoordinateError::LineOutOfBounds(Line::new(point.row().get())).into());
        }
        match self.display_row_mapping(point.row().get()) {
            RowMapping::Text(row) => self
                .wrap_snapshot
                .display_point_to_offset_with_bias(DisplayPoint::new(row, point.column()), bias),
            RowMapping::Block(placement) => Ok(placement.block.excerpt.output_range().start()),
        }
    }

    pub(super) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<ByteOffset> {
        self.display_point_to_offset_with_bias(point, FoldBias::Left)
    }

    pub(super) fn project_text_range(
        &self,
        range: TextRange,
    ) -> DisplayMapResult<Vec<ProjectedRange>> {
        self.wrap_snapshot
            .project_text_range(range)?
            .into_iter()
            .map(|range| {
                let start = range.start();
                let end = range.end();
                ProjectedRange::new(
                    ProjectedPoint::new(
                        ProjectedLineIndex::new(self.wrap_row_to_display_row(start.line().get())),
                        start.column(),
                    ),
                    ProjectedPoint::new(
                        ProjectedLineIndex::new(self.wrap_row_to_display_row(end.line().get())),
                        end.column(),
                    ),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    fn excerpt_for_line<'a>(
        &'a self,
        line: usize,
        excerpt_index: &mut Option<usize>,
    ) -> Option<&'a ExcerptSnapshot> {
        let index = excerpt_index.get_or_insert_with(|| {
            self.excerpts
                .partition_point(|excerpt| excerpt.output_end_line() <= line)
        });
        while *index < self.excerpts.len() && self.excerpts[*index].output_end_line() <= line {
            *index += 1;
        }
        self.excerpts.get(*index).filter(|excerpt| {
            excerpt.output_start_line() <= line && line < excerpt.output_end_line()
        })
    }

    pub(super) fn line_to_display_row(&self, offset: ByteOffset) -> Option<DisplayRow> {
        self.offset_to_display_point(offset)
            .ok()
            .map(DisplayPoint::row)
    }
}
