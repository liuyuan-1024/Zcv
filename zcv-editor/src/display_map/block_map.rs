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
    /// MultiBuffer 提供的逻辑 excerpt 边界签名。
    ///
    /// 保存下标、Buffer 身份与「是否进入新 Buffer」，避免在文件集合变化而边界数量未变时复用旧块分类。
    excerpt_boundaries: Arc<[(usize, BufferId, bool)]>,
    /// 构建时的整文件折叠集合；变化会使块布局失效。
    folded_buffers: HashSet<BufferId>,
    /// 构建时的显示策略；变化会改变 header/divider 分类，必须参与失效判断。
    show_headers: bool,
    /// 块锚点（wrap 行）；换行编辑时按区间平移并重排。
    specs: Arc<[BlockSpec]>,
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

/// 既有块布局中可直接复用的前缀：变换子树及续排起点。
struct BlockLayoutPrefix {
    transforms: SumTree<Transform>,
    wrap_row: usize,
}

impl BlockLayoutPrefix {
    fn empty() -> Self {
        Self {
            transforms: SumTree::new(()),
            wrap_row: 0,
        }
    }
}

/// 构建块投影所需的输入事实；每次同步由当前换行快照与策略产生。
struct BlockProjectionInputs {
    wrap_snapshot: WrapSnapshot,
    excerpts: Arc<[ExcerptSnapshot]>,
    excerpt_boundaries: Arc<[(usize, BufferId, bool)]>,
    folded_buffers: HashSet<BufferId>,
    show_headers: bool,
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

/// 比较新旧块锚点表，返回首个变化的下标与尾部可复用的块数量。
///
/// 已折叠块吞掉的行区间由后续锚点推出，因此几何比较必须同时覆盖 `hidden_end`。
fn changed_spec_range(
    old: &[BlockSpec],
    new: &[BlockSpec],
    old_wrap_line_count: usize,
    new_wrap_line_count: usize,
) -> (usize, usize) {
    let mut first = 0;
    while first < old.len().min(new.len())
        && specs_equivalent(
            old,
            first,
            old_wrap_line_count,
            new,
            first,
            new_wrap_line_count,
        )
    {
        first += 1;
    }
    let mut suffix = 0;
    while suffix + first < old.len().min(new.len()) {
        let old_index = old.len() - 1 - suffix;
        let new_index = new.len() - 1 - suffix;
        if specs_equivalent(
            old,
            old_index,
            old_wrap_line_count,
            new,
            new_index,
            new_wrap_line_count,
        ) {
            suffix += 1;
        } else {
            break;
        }
    }
    (first, suffix)
}

fn specs_equivalent(
    old: &[BlockSpec],
    old_index: usize,
    old_wrap_line_count: usize,
    new: &[BlockSpec],
    new_index: usize,
    new_wrap_line_count: usize,
) -> bool {
    old[old_index].wrap_row == new[new_index].wrap_row
        && old[old_index].excerpt_index == new[new_index].excerpt_index
        && old[old_index].height == new[new_index].height
        && old[old_index].kind == new[new_index].kind
        && old[old_index].folded_group == new[new_index].folded_group
        && spec_hidden_end(old, old_index, old_wrap_line_count)
            == spec_hidden_end(new, new_index, new_wrap_line_count)
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

    pub(crate) fn rows(&self, start_row: DisplayRow, line_count: usize) -> BlockRows<'_> {
        BlockRows::new(self, start_row, line_count)
    }

    pub(super) fn new(
        wrap_snapshot: WrapSnapshot,
        excerpts: Arc<[ExcerptSnapshot]>,
        folded_buffers: &HashSet<BufferId>,
    ) -> Self {
        let show_headers = wrap_snapshot.buffer_snapshot().show_headers();
        let excerpt_boundaries = excerpt_boundaries(&wrap_snapshot);
        let specs: Arc<[BlockSpec]> = compute_specs(
            &wrap_snapshot,
            &excerpts,
            &excerpt_boundaries,
            folded_buffers,
            show_headers,
        )
        .into();
        let spec_count = specs.len();
        Self::rebuild(
            BlockLayoutPrefix::empty(),
            specs,
            0,
            spec_count,
            None,
            BlockProjectionInputs {
                wrap_snapshot,
                excerpts,
                excerpt_boundaries,
                folded_buffers: folded_buffers.clone(),
                show_headers,
            },
        )
    }

    /// 由块锚点表构建 WrapRow→BlockRow 变换树。
    ///
    /// `prefix` 提供 `spec_start` 之前可直接复用的变换；`spec_end` 之后若提供 `suffix`，
    /// 直接追加既有变换子树（仅在输入空间未变时合法）。函数只重建变化区间，不整体重建。
    fn rebuild(
        mut prefix: BlockLayoutPrefix,
        specs: Arc<[BlockSpec]>,
        spec_start: usize,
        spec_end: usize,
        suffix: Option<SumTree<Transform>>,
        inputs: BlockProjectionInputs,
    ) -> Self {
        let BlockProjectionInputs {
            wrap_snapshot,
            excerpts,
            excerpt_boundaries,
            folded_buffers,
            show_headers,
        } = inputs;
        let wrap_line_count = wrap_snapshot.line_count();
        let mut wrap_row = prefix.wrap_row;
        for index in spec_start..spec_end {
            let spec = &specs[index];
            let spec_wrap_row = spec.wrap_row.min(wrap_line_count);
            if wrap_row < spec_wrap_row {
                push_text_rows(&mut prefix.transforms, spec_wrap_row - wrap_row);
                wrap_row = spec_wrap_row;
            }
            let hidden_end = spec_hidden_end(specs.as_ref(), index, wrap_line_count);
            prefix.transforms.push(
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
        let target_wrap_row = if suffix.is_some() {
            specs
                .get(spec_end)
                .map_or(wrap_line_count, |spec| spec.wrap_row.min(wrap_line_count))
        } else {
            wrap_line_count
        };
        if wrap_row < target_wrap_row {
            push_text_rows(&mut prefix.transforms, target_wrap_row - wrap_row);
        }
        if let Some(suffix) = suffix {
            prefix.transforms.append(suffix, ());
        }
        debug_assert_eq!(
            prefix.transforms.summary().input_rows,
            wrap_line_count,
            "块投影变换的输入行必须精确覆盖换行投影"
        );
        Self {
            wrap_snapshot,
            transforms: prefix.transforms,
            excerpts,
            excerpt_boundaries,
            folded_buffers,
            show_headers,
            specs,
        }
    }

    /// 消费换行编辑流，返回推进后的块投影。
    ///
    /// 与 Zed `BlockMap::sync` 一致，读取路径总是同步推进：无结构变化且无换行编辑时复用整棵变换树；
    /// 换行编辑或折叠策略变化时只重建变化区间，未受影响的变换子树按 Arc 复用，不整体重建。
    pub(super) fn sync(
        &self,
        wrap_snapshot: WrapSnapshot,
        excerpts: Arc<[ExcerptSnapshot]>,
        folded_buffers: &HashSet<BufferId>,
        wrap_edits: &[WrapEdit],
    ) -> BlockSnapshot {
        let show_headers = wrap_snapshot.buffer_snapshot().show_headers();
        let excerpt_boundaries = excerpt_boundaries(&wrap_snapshot);
        let old_wrap_line_count = self.wrap_snapshot.line_count();
        let new_wrap_line_count = wrap_snapshot.line_count();
        let structural = &self.folded_buffers != folded_buffers
            || excerpt_boundaries != self.excerpt_boundaries
            || show_headers != self.show_headers;
        let inputs = BlockProjectionInputs {
            wrap_snapshot,
            excerpts,
            excerpt_boundaries,
            folded_buffers: folded_buffers.clone(),
            show_headers,
        };

        if !structural {
            if wrap_edits.is_empty() {
                return self.reuse_transforms(inputs);
            }
            let specs = self.relocate_specs(&inputs, wrap_edits);
            return self.rebuild_after_edits(
                inputs,
                specs,
                old_wrap_line_count,
                new_wrap_line_count,
            );
        }

        // 结构变化（折叠集合、显示策略或 excerpt 拓扑）需要重算块分类。
        let specs: Arc<[BlockSpec]> = compute_specs(
            &inputs.wrap_snapshot,
            &inputs.excerpts,
            &inputs.excerpt_boundaries,
            &inputs.folded_buffers,
            inputs.show_headers,
        )
        .into();

        if wrap_edits.is_empty() && inputs.excerpt_boundaries == self.excerpt_boundaries {
            // 输入空间未变：可同时复用变化区间前后的变换子树。
            let (first, suffix_len) = changed_spec_range(
                &self.specs,
                &specs,
                old_wrap_line_count,
                new_wrap_line_count,
            );
            let new_end = specs.len().saturating_sub(suffix_len);
            if first >= new_end {
                return self.reuse_transforms(inputs);
            }
            let prefix = self.prefix_before(first, old_wrap_line_count);
            // 后缀位于旧变换树中，必须用旧规格下标定位；新旧块数量可不同（折叠会合并块）。
            let old_suffix_index = self.specs.len().saturating_sub(suffix_len);
            let suffix = (new_end < specs.len())
                .then(|| self.suffix_from(old_suffix_index, old_wrap_line_count));
            return Self::rebuild(prefix, specs, first, new_end, suffix, inputs);
        }

        // excerpt 拓扑变化会平移锚点，只能复用首个变化块之前的前缀。
        let (first, _) = changed_spec_range(
            &self.specs,
            &specs,
            old_wrap_line_count,
            new_wrap_line_count,
        );
        let prefix = self.prefix_before(first, old_wrap_line_count);
        if first < specs.len() && specs[first].wrap_row < prefix.wrap_row {
            let spec_count = specs.len();
            return Self::rebuild(
                BlockLayoutPrefix::empty(),
                specs,
                0,
                spec_count,
                None,
                inputs,
            );
        }
        let spec_count = specs.len();
        Self::rebuild(prefix, specs, first, spec_count, None, inputs)
    }

    /// 复用整棵变换树，只替换片段视图与策略事实。
    fn reuse_transforms(&self, inputs: BlockProjectionInputs) -> BlockSnapshot {
        BlockSnapshot {
            wrap_snapshot: inputs.wrap_snapshot,
            transforms: self.transforms.clone(),
            excerpts: inputs.excerpts,
            excerpt_boundaries: inputs.excerpt_boundaries,
            folded_buffers: inputs.folded_buffers,
            show_headers: inputs.show_headers,
            specs: self.specs.clone(),
        }
    }

    fn relocate_specs(
        &self,
        inputs: &BlockProjectionInputs,
        wrap_edits: &[WrapEdit],
    ) -> Arc<[BlockSpec]> {
        let mut specs = self.specs.to_vec();
        for spec in &mut specs {
            spec.wrap_row = relocated_wrap_row(
                spec.wrap_row,
                spec.excerpt_index,
                wrap_edits,
                &inputs.wrap_snapshot,
                &inputs.excerpts,
            );
        }
        specs.into()
    }

    fn rebuild_after_edits(
        &self,
        inputs: BlockProjectionInputs,
        specs: Arc<[BlockSpec]>,
        old_wrap_line_count: usize,
        new_wrap_line_count: usize,
    ) -> BlockSnapshot {
        let first_changed = (0..specs.len())
            .find(|&index| {
                !specs_equivalent(
                    &self.specs,
                    index,
                    old_wrap_line_count,
                    &specs,
                    index,
                    new_wrap_line_count,
                )
            })
            .unwrap_or(specs.len());
        let prefix = self.prefix_before(first_changed, old_wrap_line_count);
        // 后缀锚点上移会越过复用前缀的终点，前缀几何不再有效；从投影起点重建。
        if first_changed < specs.len() && specs[first_changed].wrap_row < prefix.wrap_row {
            let spec_count = specs.len();
            return Self::rebuild(
                BlockLayoutPrefix::empty(),
                specs,
                0,
                spec_count,
                None,
                inputs,
            );
        }
        let spec_count = specs.len();
        Self::rebuild(prefix, specs, first_changed, spec_count, None, inputs)
    }

    /// 旧布局中前 `spec_count` 个块结束后的续排位置：换行行与显示行。
    fn layout_before(&self, spec_count: usize, wrap_line_count: usize) -> (usize, usize) {
        let mut wrap_row = 0usize;
        let mut display_row = 0usize;
        for index in 0..spec_count {
            let spec = &self.specs[index];
            if wrap_row < spec.wrap_row {
                display_row += spec.wrap_row - wrap_row;
            }
            display_row += spec.height;
            wrap_row = spec_hidden_end(self.specs.as_ref(), index, wrap_line_count);
        }
        (wrap_row, display_row)
    }

    /// 从旧变换树中截取 `spec_index` 号块起的后缀子树。
    ///
    /// 仅在输入空间未变化时合法：该位置在旧、新布局中对应同一变换边界。
    /// 块变换的输入行可为零（header/divider 不消耗文本行），因此边界取
    /// 「前一块末端 + 到该锚点的间隙文本」，而不是换行行本身。
    fn suffix_from(&self, spec_index: usize, wrap_line_count: usize) -> SumTree<Transform> {
        let (wrap_row, display_row) = self.layout_before(spec_index, wrap_line_count);
        let gap = self.specs[spec_index].wrap_row.saturating_sub(wrap_row);
        let mut cursor = self.transforms.cursor::<OutputToInput>(());
        cursor.slice(&OutputRows(display_row + gap), Bias::Right);
        cursor.suffix()
    }

    /// 复用前 `first_changed` 个块之前的变换。
    ///
    /// 每个变换的 `output_rows` 都大于零，因此按输出行切片没有零宽边界歧义；
    /// 用 `Bias::Right` 纳入正好结束于分界处的变换。
    fn prefix_before(&self, first_changed: usize, wrap_line_count: usize) -> BlockLayoutPrefix {
        let (wrap_row, display_row) = self.layout_before(first_changed, wrap_line_count);
        let mut cursor = self.transforms.cursor::<OutputToInput>(());
        BlockLayoutPrefix {
            transforms: cursor.slice(&OutputRows(display_row), Bias::Right),
            wrap_row,
        }
    }

    pub(super) fn line_count(&self) -> usize {
        self.transforms.summary().output_rows
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
