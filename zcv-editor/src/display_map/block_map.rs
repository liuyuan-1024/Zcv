//! 多文件 Editor 的块级显示投影。
//!
//! 本层位于 WrapMap 之上：文本换行坐标保持不变，文件标题和同文件片段分隔线作为不属于文本的虚拟显示块插入。
//! 这样搜索、diff、诊断等宿主只负责提供 excerpts，滚动、命中测试、选区和通用文件标题都由 Editor 复用同一条管线。

use zcv_multi_buffer::{ExcerptSnapshot, MultiBufferOffset, MultiBufferRange};

use std::collections::{BTreeSet, HashSet};
use std::ops::Range;
use std::sync::Arc;

use sum_tree::{Bias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use zcv_text::{BufferId, CoordinateError, Line};

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

/// 块投影中的一个虚拟块。
///
/// 显示行由所在变换在输出行空间的位置决定，片段来源由 `excerpt_index` 指向 `BlockSnapshot.excerpts`；
/// 这里只保存不随片段视图刷新的身份信息，使未受影响的变换子树可以跨快照复用。
#[derive(Clone, Debug, PartialEq, Eq)]
struct BlockPlacement {
    height: usize,
    kind: DisplayBlockKind,
    excerpt_index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TransformKind {
    Text,
    Block(Arc<BlockPlacement>),
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    excerpts: Arc<[ExcerptSnapshot]>,
    /// 当前快照的块锚点表（经换行行定位的派生布局）。
    ///
    /// 它不承担跨快照的拼接身份，只用于判断显示几何是否变化：
    /// 锚点表与换行行数都不变时，新的块变换树与旧树逐节点等价，可整棵复用。
    specs: Arc<[BlockSpec]>,
    /// 块几何代际：只有显示几何真正变化时推进，供依赖显示几何的缓存精确失效。
    geometry_epoch: u64,
}

fn excerpt_boundaries(wrap_snapshot: &WrapSnapshot) -> Arc<[(usize, BufferId, bool)]> {
    wrap_snapshot
        .buffer_snapshot()
        .excerpt_boundaries()
        .map(|boundary| {
            (
                boundary.next_index(),
                boundary.next().buffer_id(),
                boundary.starts_new_buffer(),
            )
        })
        .collect()
}

/// 决定一个逻辑 excerpt 边界放置实体 header、divider，还是不放置块。
///
/// 对齐 Zed `BlockMap::header_and_footer_blocks` 的分类：
/// 进入新 Buffer 且显示策略允许时画实体 header；
/// 否则只有文档首个 excerpt 之后的边界画 divider；
/// 文档首个 excerpt 在没有 header 策略时不产生块。
fn entry_block_kind(
    show_headers: bool,
    is_document_start: bool,
    index_in_buffer: usize,
) -> Option<DisplayBlockKind> {
    if index_in_buffer > 0 {
        return Some(DisplayBlockKind::ExcerptBoundary);
    }
    if show_headers {
        Some(DisplayBlockKind::BufferHeader)
    } else if is_document_start {
        None
    } else {
        Some(DisplayBlockKind::ExcerptBoundary)
    }
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
        match &transform.kind {
            TransformKind::Block(placement) => {
                let height = placement.height;
                let kind = placement.kind;
                let excerpt_index = placement.excerpt_index;
                let display_row = transform_start.0.0;
                self.row = (display_row + height).min(self.end);
                self.block_cursor.next();
                self.seek_wrap_to_current_transform();
                Some(BlockRow {
                    index: DisplayRow::new(display_row),
                    height,
                    kind: BlockRowKind::Block(DisplayBlock {
                        kind,
                        excerpt: self.snapshot.excerpts[excerpt_index].clone(),
                    }),
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

/// 构建块投影所需的输入事实；每次同步由当前换行快照与策略产生。
struct BlockProjectionInputs {
    wrap_snapshot: WrapSnapshot,
    excerpts: Arc<[ExcerptSnapshot]>,
    specs: Arc<[BlockSpec]>,
}

/// BlockMap 消费的下层换行 patch。
///
/// `WrapMap` 正常路径已经提供逐区间编辑；
/// 块策略或组合投影结构变化没有可复用的行级身份，因此明确表示为一次覆盖全范围的替换。
/// 这样 BlockMap 不再从块规格的下标猜测同步范围，所有变换树重建都以旧/新 Wrap 输入空间的显式 patch 为边界。
#[derive(Clone, Debug, Default)]
struct BlockPatch {
    edits: Vec<WrapEdit>,
}

impl BlockPatch {
    fn from_wrap_edits(
        wrap_edits: &[WrapEdit],
        old_line_count: usize,
        new_line_count: usize,
        structural: bool,
    ) -> Self {
        if structural {
            return Self {
                edits: vec![WrapEdit {
                    old: 0..old_line_count,
                    new: 0..new_line_count,
                }],
            };
        }
        Self {
            edits: wrap_edits.to_vec(),
        }
    }

    fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }
}

/// 从片段边界与折叠策略推导块锚点表；输入或策略变化时整体重算。
fn compute_specs(
    wrap_snapshot: &WrapSnapshot,
    excerpts: &[ExcerptSnapshot],
    excerpt_boundaries: &[(usize, BufferId, bool)],
    folded_buffers: &HashSet<BufferId>,
    show_headers: bool,
) -> Vec<BlockSpec> {
    let excerpt_starts = excerpt_boundaries
        .iter()
        .enumerate()
        .filter_map(|(boundary_index, (excerpt_index, _, _))| {
            wrap_snapshot
                .offset_to_wrap_point(excerpts[*excerpt_index].output_range().start())
                .ok()
                .map(|point| (point.row().get(), boundary_index))
        })
        .collect::<Vec<_>>();
    let mut specs = Vec::new();
    let mut group_start = 0usize;
    while group_start < excerpt_starts.len() {
        let (_, buffer_id, _) = excerpt_boundaries[excerpt_starts[group_start].1];
        // 逻辑边界只有在 `starts_new_buffer` 处才开启新组；同组的后续边界是同一文件的后续窗口。
        let mut group_end = group_start + 1;
        while group_end < excerpt_starts.len() && !excerpt_boundaries[excerpt_starts[group_end].1].2
        {
            group_end += 1;
        }
        // 相邻逻辑 excerpt 共享 Buffer 身份时只是同一文件的后续窗口，不进入新 Buffer 边界。
        // 整文件折叠只在显示策略允许 header 时折叠为一整块；否则按普通 divider 序列绘制。
        if folded_buffers.contains(&buffer_id) && show_headers {
            let (wrap_row, boundary_index) = excerpt_starts[group_start];
            specs.push(BlockSpec {
                wrap_row,
                excerpt_index: excerpt_boundaries[boundary_index].0,
                height: FILE_HEADER_HEIGHT,
                kind: DisplayBlockKind::BufferHeader,
                folded_group: true,
            });
        } else {
            let is_document_start = group_start == 0;
            for (index, (wrap_row, boundary_index)) in
                excerpt_starts[group_start..group_end].iter().enumerate()
            {
                let Some(kind) = entry_block_kind(show_headers, is_document_start, index) else {
                    continue;
                };
                specs.push(BlockSpec {
                    wrap_row: *wrap_row,
                    excerpt_index: excerpt_boundaries[*boundary_index].0,
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
    specs
}

/// 从当前换行快照与块策略派生整份块锚点表。
fn derive_specs(
    wrap_snapshot: &WrapSnapshot,
    excerpts: &[ExcerptSnapshot],
    folded_buffers: &HashSet<BufferId>,
    show_headers: bool,
) -> Arc<[BlockSpec]> {
    compute_specs(
        wrap_snapshot,
        excerpts,
        &excerpt_boundaries(wrap_snapshot),
        folded_buffers,
        show_headers,
    )
    .into()
}

fn push_text_rows(transforms: &mut SumTree<Transform>, rows: usize) {
    if rows > 0 {
        transforms.push(
            Transform {
                kind: TransformKind::Text,
                input_rows: rows,
                output_rows: rows,
            },
            (),
        );
    }
}

impl BlockSnapshot {
    pub(super) fn wrap_snapshot(&self) -> &WrapSnapshot {
        &self.wrap_snapshot
    }

    /// 块几何代际；变换树整棵复用时保持不变。
    pub(super) fn geometry_epoch(&self) -> u64 {
        self.geometry_epoch
    }

    pub(crate) fn rows(&self, start_row: DisplayRow, line_count: usize) -> BlockRows<'_> {
        BlockRows::new(self, start_row, line_count)
    }

    pub(super) fn new(
        wrap_snapshot: WrapSnapshot,
        excerpts: Arc<[ExcerptSnapshot]>,
        folded_buffers: &HashSet<BufferId>,
    ) -> Self {
        let show_headers = wrap_snapshot.buffer_snapshot().show_headers();
        let specs = derive_specs(&wrap_snapshot, &excerpts, folded_buffers, show_headers);
        Self::rebuild_from_snapshot(
            0,
            BlockProjectionInputs {
                wrap_snapshot,
                excerpts,
                specs,
            },
        )
    }

    /// 根据当前 WrapSnapshot 的权威边界事实完整物化块变换树。
    ///
    /// BlockSpec 是当前快照的局部派生，不承担跨快照同步身份；
    /// 跨快照的变化范围由 [`BlockPatch`] 表达。
    /// 这样不会把旧规格下标或旧变换树片段混入新快照。
    fn rebuild_from_snapshot(geometry_epoch: u64, inputs: BlockProjectionInputs) -> Self {
        let BlockProjectionInputs {
            wrap_snapshot,
            excerpts,
            specs,
        } = inputs;
        let wrap_line_count = wrap_snapshot.line_count();
        let mut transforms = SumTree::new(());
        let mut wrap_row = 0usize;
        for index in 0..specs.len() {
            let spec = &specs[index];
            let spec_wrap_row = spec.wrap_row.min(wrap_line_count);
            if wrap_row < spec_wrap_row {
                push_text_rows(&mut transforms, spec_wrap_row - wrap_row);
                wrap_row = spec_wrap_row;
            }
            let hidden_end = spec_hidden_end(&specs, index, wrap_line_count);
            transforms.push(
                Transform {
                    kind: TransformKind::Block(Arc::new(BlockPlacement {
                        height: spec.height,
                        kind: spec.kind,
                        excerpt_index: spec.excerpt_index,
                    })),
                    input_rows: hidden_end.saturating_sub(wrap_row),
                    output_rows: spec.height,
                },
                (),
            );
            wrap_row = hidden_end;
        }
        if wrap_row < wrap_line_count {
            push_text_rows(&mut transforms, wrap_line_count - wrap_row);
        }
        let snapshot = Self {
            wrap_snapshot,
            transforms,
            excerpts,
            specs,
            geometry_epoch,
        };
        snapshot.check_invariants();
        snapshot
    }

    /// 消费显式 Wrap patch，返回推进后的块投影。
    ///
    /// 与 Zed `BlockMap::sync` 相同，BlockMap 只从下层 patch 判断是否需要推进；不再以 `BlockSpec` 下标推断旧树的拼接区间。
    /// Zcv 当前的块种类都由当前excerpt 边界派生，因此一个非空 patch 直接物化当前快照的完整块变换树，让块的输入空间始终只属于这一份 WrapSnapshot。
    pub(super) fn sync(
        &self,
        wrap_snapshot: WrapSnapshot,
        excerpts: Arc<[ExcerptSnapshot]>,
        folded_buffers: &HashSet<BufferId>,
        wrap_edits: &[WrapEdit],
    ) -> BlockSnapshot {
        let show_headers = wrap_snapshot.buffer_snapshot().show_headers();
        let old_wrap_line_count = self.wrap_snapshot.line_count();
        let new_wrap_line_count = wrap_snapshot.line_count();
        let specs = derive_specs(&wrap_snapshot, &excerpts, folded_buffers, show_headers);
        // 显示几何只由块锚点表与换行行数决定：
        // 两者都未变时新树与旧树逐节点等价，不得推进几何代际，否则同一行内编辑会错误地使 diff 装饰等依赖显示几何的缓存失效。
        let geometry_changed =
            specs.as_ref() != self.specs.as_ref() || old_wrap_line_count != new_wrap_line_count;
        let inputs = BlockProjectionInputs {
            wrap_snapshot,
            excerpts,
            specs,
        };
        let patch = BlockPatch::from_wrap_edits(
            wrap_edits,
            old_wrap_line_count,
            new_wrap_line_count,
            geometry_changed,
        );
        if patch.is_empty() {
            return self.reuse_transforms(inputs);
        }
        let geometry_epoch = if geometry_changed {
            self.geometry_epoch + 1
        } else {
            self.geometry_epoch
        };
        Self::rebuild_from_snapshot(geometry_epoch, inputs)
    }

    /// 复用整棵变换树，只替换片段视图与策略事实。
    fn reuse_transforms(&self, inputs: BlockProjectionInputs) -> BlockSnapshot {
        let snapshot = BlockSnapshot {
            wrap_snapshot: inputs.wrap_snapshot,
            transforms: self.transforms.clone(),
            excerpts: inputs.excerpts,
            specs: inputs.specs,
            geometry_epoch: self.geometry_epoch,
        };
        snapshot.check_invariants();
        snapshot
    }

    pub(super) fn line_count(&self) -> usize {
        self.transforms.summary().output_rows
    }

    /// 块层的输入必须完整且仅一次地覆盖它所消费的换行投影。
    ///
    /// 这是 DisplayMap 层间快照一致性的边界检查；
    /// 不在坐标查询时才暴露漂移。
    fn check_invariants(&self) {
        debug_assert_eq!(
            self.transforms.summary().input_rows,
            self.wrap_snapshot.line_count(),
            "块投影变换的输入行必须精确覆盖换行投影"
        );
    }

    /// 返回视口顶部所在 excerpt 的文件标题，以及下一个文件标题的位置。
    ///
    /// 同一文件的后续 excerpt 只有分隔块，但它同样会更新标题所代表的 excerpt，使“打开文件”等操作仍以当前可见片段为目标。
    pub(super) fn sticky_buffer_header(&self, top_row: DisplayRow) -> Option<StickyBufferHeader> {
        let mut cursor = self.transforms.cursor::<OutputToInput>(());
        cursor.seek(&OutputRows(0), Bias::Left);
        let mut current: Option<(usize, usize)> = None;
        let mut next_buffer_header_row: Option<usize> = None;
        while let Some(transform) = cursor.item() {
            let display_row = cursor.start().0.0;
            let block = match &transform.kind {
                TransformKind::Block(placement) => Some((placement.kind, placement.excerpt_index)),
                TransformKind::Text => None,
            };
            if let Some((kind, excerpt_index)) = block {
                if display_row <= top_row.get() {
                    current = Some((display_row, excerpt_index));
                    next_buffer_header_row = None;
                } else if current.is_some()
                    && next_buffer_header_row.is_none()
                    && kind == DisplayBlockKind::BufferHeader
                {
                    next_buffer_header_row = Some(display_row);
                    break;
                }
            }
            cursor.next();
        }
        let (source_row, excerpt_index) = current?;
        Some(StickyBufferHeader {
            source_row: DisplayRow::new(source_row),
            excerpt: self.excerpts[excerpt_index].clone(),
            next_buffer_header_row: next_buffer_header_row.map(DisplayRow::new),
        })
    }

    fn wrap_row_to_display_row(&self, wrap_row: usize) -> usize {
        assert!(
            wrap_row < self.transforms.summary().input_rows,
            "换行行 {wrap_row} 超出块投影输入范围 {}",
            self.transforms.summary().input_rows
        );
        let (start, _, transform) =
            self.transforms
                .find::<InputToOutput, _>((), &InputRows(wrap_row), Bias::Right);
        match transform.map(|transform| &transform.kind) {
            Some(TransformKind::Text) => start.1.0 + wrap_row - start.0.0,
            Some(TransformKind::Block(_)) => start.1.0,
            None => unreachable!("块投影变换必须精确覆盖换行投影的每一行"),
        }
    }

    pub(super) fn projected_wrap_row_to_display_row(&self, wrap_row: usize) -> DisplayRow {
        DisplayRow::new(self.wrap_row_to_display_row(wrap_row))
    }

    fn display_row_mapping(&self, display_row: usize) -> RowMapping<'_> {
        let (start, _, transform) =
            self.transforms
                .find::<OutputToInput, _>((), &OutputRows(display_row), Bias::Right);
        match transform.map(|transform| &transform.kind) {
            Some(TransformKind::Text) => RowMapping::Text(WrapRow::new(
                start.1.0 + display_row.saturating_sub(start.0.0),
            )),
            Some(TransformKind::Block(placement)) => RowMapping::Block(placement),
            None => unreachable!("显示行必须落在块投影变换区间内"),
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
            RowMapping::Block(placement) => Ok(self.excerpts[placement.excerpt_index]
                .output_range()
                .start()),
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

#[cfg(test)]
#[path = "test/block_map_tests.rs"]
mod tests;
