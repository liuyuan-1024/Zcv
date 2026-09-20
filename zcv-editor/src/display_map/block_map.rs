//! 多文件 Editor 的块级显示投影。
//!
//! 本层位于 WrapMap 之上：文本换行坐标保持不变，文件标题和同文件片段分隔线作为不属于文本的虚拟显示块插入。
//! 这样搜索、diff、诊断等宿主只负责提供 excerpts，滚动、命中测试、选区和通用文件标题都由 Editor 复用同一条管线。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::collections::{BTreeSet, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use sum_tree::{Bias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use zcv_multi_buffer::ExcerptSnapshot;
use zcv_text::{CoordinateError, Line};

use super::error::DisplayMapResult;
use super::fold_map::{FoldBias, ProjectedLineIndex};
use super::wrap_map::{WrapEdit, WrapRowKind, WrapRows, WrapSnapshot};
use super::{DisplayColumn, DisplayPoint, DisplayRange, DisplayRow, WrapPoint, WrapRow};

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
    /// 该块来自的片段下标；换行布局未变时据此刷新 `block.excerpt`。
    excerpt_index: usize,
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

/// 块的换行行锚点；与 [`BlockPlacement`] 分离，使换行重排只需平移锚点而无需重算片段分组。
#[derive(Debug, Clone)]
struct BlockSpec {
    wrap_row: usize,
    excerpt_index: usize,
    height: usize,
    kind: DisplayBlockKind,
    /// 整文件折叠组：该块吞掉到下一个块之前的全部换行行。
    folded_group: bool,
}

#[derive(Debug, Clone)]
pub(super) struct BlockSnapshot {
    wrap_snapshot: WrapSnapshot,
    transforms: SumTree<Transform>,
    placements: Vec<BlockPlacement>,
    excerpts: Arc<[ExcerptSnapshot]>,
    /// 块起始片段在 `excerpts` 中的下标；换行布局未变时据此刷新块视图。
    block_start_indices: Arc<[usize]>,
    /// 构建时的整文件折叠集合；变化会使块布局失效。
    folded_buffers: HashSet<PathBuf>,
    /// 块锚点（wrap 行）；换行编辑时按区间平移并重排。
    specs: Arc<[BlockSpec]>,
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
                        .excerpt_for_line(source.get(), &mut self.excerpt_index)
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
                    {
                        lines.insert(stream_line);
                    }
                }
            } else {
                lines.insert(source);
            }
        }
        let mut ranges = Vec::new();
        let mut lines = lines.into_iter().peekable();
        while let Some(start) = lines.next() {
            let mut end = start;
            while lines.peek().is_some_and(|line| line.get() == end.get() + 1) {
                end = lines.next().expect("连续源行的下一个元素必须存在");
            }
            ranges.push(start..Line::new(end.get() + 1));
        }
        ranges
    }
}

enum RowMapping<'a> {
    Text(WrapRow),
    Block(&'a BlockPlacement),
}

/// 把块锚点从旧换行行坐标平移到新坐标；锚点落入编辑区间时用新换行快照重算。
///
/// 块锚点一定位于其 excerpt 输出起点之后，且该起点必须可解析到换行投影；
/// 两者由物化与换行同步的契约保证。
/// 失败说明投影不一致，直接失败而不是让整份块投影重建。
fn relocated_wrap_row(
    old_row: usize,
    excerpt_index: usize,
    wrap_edits: &[WrapEdit],
    wrap_snapshot: &WrapSnapshot,
    excerpts: &[ExcerptSnapshot],
) -> usize {
    let mut row = old_row as isize;
    for edit in wrap_edits {
        if row < edit.old.start as isize {
            break;
        }
        if row >= edit.old.end as isize {
            row +=
                (edit.new.end - edit.new.start) as isize - (edit.old.end - edit.old.start) as isize;
        } else {
            return wrap_snapshot
                .offset_to_wrap_point(excerpts[excerpt_index].output_range().start())
                .expect("块锚点重定位时 excerpt 输出起点必须可解析到换行投影")
                .row()
                .get();
        }
    }
    assert!(row >= 0, "块锚点重定位不得落到换行投影起点之前");
    row as usize
}

/// 块在换行投影中吞掉的行区间终点。
///
/// 整文件折叠块延续到下一个块（或投影末尾），普通块不隐藏任何换行行。
fn spec_hidden_end(specs: &[BlockSpec], index: usize, wrap_line_count: usize) -> usize {
    let spec = &specs[index];
    if spec.folded_group {
        specs
            .get(index + 1)
            .map_or(wrap_line_count, |next| next.wrap_row.min(wrap_line_count))
    } else {
        spec.wrap_row
    }
}

/// 既有块布局中可直接复用的前缀：变换子树、块位置及续排起点。
struct BlockLayoutPrefix {
    transforms: SumTree<Transform>,
    placements: Vec<BlockPlacement>,
    wrap_row: usize,
    display_row: usize,
}

impl BlockLayoutPrefix {
    fn empty() -> Self {
        Self {
            transforms: SumTree::new(()),
            placements: Vec::new(),
            wrap_row: 0,
            display_row: 0,
        }
    }
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
        excerpts: Arc<[ExcerptSnapshot]>,
        folded_buffers: &HashSet<PathBuf>,
    ) -> Self {
        let excerpt_starts = excerpts
            .iter()
            .enumerate()
            .filter(|(_, excerpt)| excerpt.starts_new_excerpt())
            .filter_map(|(index, excerpt)| {
                wrap_snapshot
                    .offset_to_wrap_point(excerpt.output_range().start())
                    .ok()
                    .map(|point| (point.row().get(), index))
            })
            .collect::<Vec<_>>();
        let block_start_indices: Arc<[usize]> =
            excerpt_starts.iter().map(|(_, index)| *index).collect();
        let mut specs = Vec::new();
        let mut group_start = 0usize;
        while group_start < excerpt_starts.len() {
            let path = excerpts[excerpt_starts[group_start].1].path();
            let mut group_end = group_start + 1;
            while group_end < excerpt_starts.len()
                && excerpts[excerpt_starts[group_end].1].path() == path
            {
                group_end += 1;
            }
            if folded_buffers.contains(path) {
                let (wrap_row, excerpt_index) = excerpt_starts[group_start];
                specs.push(BlockSpec {
                    wrap_row,
                    excerpt_index,
                    height: FILE_HEADER_HEIGHT,
                    kind: DisplayBlockKind::BufferHeader,
                    folded_group: true,
                });
            } else {
                for (index, (wrap_row, excerpt_index)) in
                    excerpt_starts[group_start..group_end].iter().enumerate()
                {
                    let kind = if index == 0 {
                        DisplayBlockKind::BufferHeader
                    } else {
                        DisplayBlockKind::ExcerptBoundary
                    };
                    specs.push(BlockSpec {
                        wrap_row: *wrap_row,
                        excerpt_index: *excerpt_index,
                        height: match kind {
                            DisplayBlockKind::BufferHeader => FILE_HEADER_HEIGHT,
                            DisplayBlockKind::ExcerptBoundary => EXCERPT_BOUNDARY_HEIGHT,
                        },
                        kind,
                        folded_group: false,
                    });
                }
            }
            group_start = group_end;
        }

        specs.sort_by_key(|spec| spec.wrap_row);
        Self::place_from(
            BlockLayoutPrefix::empty(),
            specs.into(),
            0,
            wrap_snapshot,
            excerpts,
            block_start_indices,
            folded_buffers.clone(),
        )
    }

    /// 由块锚点表构建 placements 与 WrapRow→BlockRow 变换树。
    ///
    /// `spec_start` 之前的前缀由 `prefix` 提供，函数从其后继续排布；放置下标随后缀继续增长。
    fn place_from(
        mut prefix: BlockLayoutPrefix,
        specs: Arc<[BlockSpec]>,
        spec_start: usize,
        wrap_snapshot: WrapSnapshot,
        excerpts: Arc<[ExcerptSnapshot]>,
        block_start_indices: Arc<[usize]>,
        folded_buffers: HashSet<PathBuf>,
    ) -> Self {
        let wrap_line_count = wrap_snapshot.line_count();
        let mut wrap_row = prefix.wrap_row;
        let mut display_row = prefix.display_row;
        for index in spec_start..specs.len() {
            let spec = &specs[index];
            let spec_wrap_row = spec.wrap_row.min(wrap_line_count);
            if wrap_row < spec_wrap_row {
                let row_count = spec_wrap_row - wrap_row;
                prefix.transforms.push(
                    Transform {
                        kind: TransformKind::Text,
                        input_rows: row_count,
                        output_rows: row_count,
                    },
                    (),
                );
                wrap_row = spec_wrap_row;
                display_row += row_count;
            }
            let placement_index = prefix.placements.len();
            prefix.placements.push(BlockPlacement {
                display_row,
                height: spec.height,
                block: DisplayBlock {
                    kind: spec.kind,
                    excerpt: excerpts[spec.excerpt_index].clone(),
                },
                excerpt_index: spec.excerpt_index,
                next_buffer_header_row: None,
            });
            let hidden_end = spec_hidden_end(specs.as_ref(), index, wrap_line_count);
            prefix.transforms.push(
                Transform {
                    kind: TransformKind::Block(placement_index),
                    input_rows: hidden_end.saturating_sub(wrap_row),
                    output_rows: spec.height,
                },
                (),
            );
            wrap_row = hidden_end;
            display_row += spec.height;
        }
        if wrap_row < wrap_line_count {
            let row_count = wrap_line_count - wrap_row;
            prefix.transforms.push(
                Transform {
                    kind: TransformKind::Text,
                    input_rows: row_count,
                    output_rows: row_count,
                },
                (),
            );
        }

        let mut next_buffer_header_row = None;
        for placement in prefix.placements.iter_mut().rev() {
            placement.next_buffer_header_row = next_buffer_header_row;
            if placement.block.kind == DisplayBlockKind::BufferHeader {
                next_buffer_header_row = Some(placement.display_row);
            }
        }

        Self {
            wrap_snapshot,
            transforms: prefix.transforms,
            placements: prefix.placements,
            excerpts,
            block_start_indices,
            folded_buffers,
            specs,
        }
    }

    /// 消费换行编辑流，返回推进后的块投影。
    ///
    /// 与 Zed `BlockMap::sync` 一致，读取路径总是同步：换行布局未变时复用变换与块位置，
    /// 只有确实改变几何的分支才整体重建。
    pub(super) fn sync(
        &self,
        wrap_snapshot: WrapSnapshot,
        excerpts: Arc<[ExcerptSnapshot]>,
        folded_buffers: &HashSet<PathBuf>,
        wrap_edits: &[WrapEdit],
    ) -> BlockSnapshot {
        let block_start_indices: Arc<[usize]> = excerpts
            .iter()
            .enumerate()
            .filter(|(_, excerpt)| excerpt.starts_new_excerpt())
            .map(|(index, _)| index)
            .collect();

        // 折叠集合或 excerpt 起始集合变化会改变块列表本身，几何增量不再适用。
        if &self.folded_buffers != folded_buffers || block_start_indices != self.block_start_indices
        {
            return Self::new(wrap_snapshot, excerpts, folded_buffers);
        }

        // 换行布局未变：复用变换与块位置，只刷新片段视图。
        if wrap_edits.is_empty() {
            let mut placements = self.placements.clone();
            for placement in &mut placements {
                placement.block.excerpt = excerpts[placement.excerpt_index].clone();
            }
            return Self {
                wrap_snapshot,
                transforms: self.transforms.clone(),
                placements,
                excerpts,
                block_start_indices,
                folded_buffers: folded_buffers.clone(),
                specs: self.specs.clone(),
            };
        }

        // 换行布局变化：按编辑平移块锚点，落入编辑区间的锚点按新换行快照重算。
        let mut specs = self.specs.to_vec();
        for spec in &mut specs {
            spec.wrap_row = relocated_wrap_row(
                spec.wrap_row,
                spec.excerpt_index,
                wrap_edits,
                &wrap_snapshot,
                &excerpts,
            );
        }

        let old_wrap_line_count = self.wrap_snapshot.line_count();
        let new_wrap_line_count = wrap_snapshot.line_count();
        // 找到第一个几何发生变化的块：锚点或它吞掉的换行区间任一改变都算。
        // 此前的块在旧、新布局中完全一致，可以直接复用其变换子树。
        let first_changed = (0..specs.len())
            .find(|&index| {
                self.specs[index].wrap_row != specs[index].wrap_row
                    || spec_hidden_end(self.specs.as_ref(), index, old_wrap_line_count)
                        != spec_hidden_end(specs.as_slice(), index, new_wrap_line_count)
            })
            .unwrap_or(specs.len());
        let mut prefix = self.prefix_before(first_changed, old_wrap_line_count);
        for placement in &mut prefix.placements {
            placement.block.excerpt = excerpts[placement.excerpt_index].clone();
        }
        // 后缀锚点上移会越过复用前缀的终点，前缀几何不再有效，整体重建。
        if first_changed < specs.len() && specs[first_changed].wrap_row < prefix.wrap_row {
            return Self::new(wrap_snapshot, excerpts, folded_buffers);
        }

        Self::place_from(
            prefix,
            specs.into(),
            first_changed,
            wrap_snapshot,
            excerpts,
            block_start_indices,
            folded_buffers.clone(),
        )
    }

    /// 复用前 `first_changed` 个块之前的变换与放置。
    ///
    /// 每个变换的 `output_rows` 都大于零，因此按输出行切片没有零宽边界歧义；
    /// 用 `Bias::Right` 纳入正好结束于分界处的变换。
    fn prefix_before(&self, first_changed: usize, wrap_line_count: usize) -> BlockLayoutPrefix {
        let mut wrap_row = 0usize;
        let mut display_row = 0usize;
        for index in 0..first_changed {
            let spec = &self.specs[index];
            if wrap_row < spec.wrap_row {
                display_row += spec.wrap_row - wrap_row;
            }
            display_row += spec.height;
            wrap_row = spec_hidden_end(self.specs.as_ref(), index, wrap_line_count);
        }
        let mut cursor = self.transforms.cursor::<OutputToInput>(());
        BlockLayoutPrefix {
            transforms: cursor.slice(&OutputRows(display_row), Bias::Right),
            placements: self.placements[..first_changed].to_vec(),
            wrap_row,
            display_row,
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

    pub(super) fn display_row_to_wrap_row(&self, display_row: DisplayRow) -> Option<WrapRow> {
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
            Some(TransformKind::Text) => RowMapping::Text(WrapRow::new(
                start.1.0 + display_row.saturating_sub(start.0.0),
            )),
            Some(TransformKind::Block(placement)) => RowMapping::Block(&self.placements[placement]),
            None => RowMapping::Text(WrapRow::ZERO),
        }
    }

    pub(super) fn offset_to_display_point(
        &self,
        offset: MultiBufferOffset,
    ) -> DisplayMapResult<DisplayPoint> {
        let point = self.wrap_snapshot.offset_to_wrap_point(offset)?;
        Ok(DisplayPoint::new(
            DisplayRow::new(self.wrap_row_to_display_row(point.row().get())),
            point.column(),
        ))
    }

    pub(super) fn display_point_to_offset_with_bias(
        &self,
        point: DisplayPoint,
        bias: FoldBias,
    ) -> DisplayMapResult<MultiBufferOffset> {
        if point.row().get() >= self.line_count() {
            return Err(CoordinateError::LineOutOfBounds(Line::new(point.row().get())).into());
        }
        match self.display_row_mapping(point.row().get()) {
            RowMapping::Text(row) => self
                .wrap_snapshot
                .wrap_point_to_offset_with_bias(WrapPoint::new(row, point.column()), bias),
            RowMapping::Block(placement) => Ok(placement.block.excerpt.output_range().start()),
        }
    }

    pub(super) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<MultiBufferOffset> {
        self.display_point_to_offset_with_bias(point, FoldBias::Left)
    }

    pub(super) fn project_text_range(
        &self,
        range: MultiBufferRange,
    ) -> DisplayMapResult<Vec<DisplayRange>> {
        Ok(self
            .wrap_snapshot
            .project_text_range(range)?
            .into_iter()
            .map(|range| {
                let start = range.start();
                let end = range.end();
                DisplayRange::new(
                    DisplayPoint::new(
                        DisplayRow::new(self.wrap_row_to_display_row(start.line().get())),
                        DisplayColumn::new(start.column().get()),
                    ),
                    DisplayPoint::new(
                        DisplayRow::new(self.wrap_row_to_display_row(end.line().get())),
                        DisplayColumn::new(end.column().get()),
                    ),
                )
            })
            .collect())
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

    pub(super) fn line_to_display_row(&self, offset: MultiBufferOffset) -> Option<DisplayRow> {
        self.offset_to_display_point(offset)
            .ok()
            .map(DisplayPoint::row)
    }
}
