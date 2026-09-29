//! 多文件 Editor 的块级显示投影。
//!
//! 本层位于 WrapMap 之上：文本换行坐标保持不变，文件标题和同文件片段分隔线作为不属于文本的虚拟显示块插入。
//! 这样搜索、diff、诊断等宿主只负责提供 excerpts，滚动、命中测试、选区和通用文件标题都由 Editor 复用同一条管线。

use zcv_multi_buffer::{ExcerptSnapshot, MultiBufferAnchor, MultiBufferOffset, MultiBufferRange};

use std::collections::{BTreeSet, HashSet};
use std::ops::Range;
use std::sync::Arc;

use sum_tree::{Bias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use zcv_text::{Affinity, BufferId, CoordinateError, Line};

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
/// `source_row` 标识它对应的真实边界块；结构只是一帧投影，不保存当前文件状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StickyBufferHeader {
    pub(crate) source_row: DisplayRow,
    pub(crate) excerpt: ExcerptSnapshot,
}

/// 块投影中的一个虚拟块。
///
/// 显示行由所在变换在输出行空间的位置决定；片段身份使用源 Anchor。
/// 当前片段元数据在消费时从对应快照解析，不把整份 excerpt 表复制进块投影。
#[derive(Clone, Debug, PartialEq, Eq)]
struct BlockPlacement {
    height: usize,
    kind: DisplayBlockKind,
    anchor: MultiBufferAnchor,
    folded: bool,
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
            expanded_headers: usize::from(
                matches!(&self.kind, TransformKind::Block(block) if block.kind == DisplayBlockKind::BufferHeader && !block.folded),
            ),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TransformSummary {
    input_rows: usize,
    output_rows: usize,
    expanded_headers: usize,
}

impl ContextLessSummary for TransformSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.input_rows += summary.input_rows;
        self.output_rows += summary.output_rows;
        self.expanded_headers += summary.expanded_headers;
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

/// 当前受影响范围内的块规格，不作为跨快照状态保存。
struct BlockSpec {
    wrap_row: usize,
    hidden_end: usize,
    placement: Arc<BlockPlacement>,
}

#[derive(Debug, Clone)]
pub(super) struct BlockSnapshot {
    wrap_snapshot: WrapSnapshot,
    transforms: SumTree<Transform>,
    folded_buffers: Arc<HashSet<BufferId>>,
    show_headers: bool,
    geometry_epoch: u64,
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
        let wrap_start = block_cursor
            .item()
            .map(|transform| {
                let transform_start = *block_cursor.start();
                match &transform.kind {
                    TransformKind::Text => {
                        transform_start.1.0 + start.saturating_sub(transform_start.0.0)
                    }
                    TransformKind::Block(_) => transform_start.1.0,
                }
            })
            .unwrap_or(snapshot.wrap_snapshot.line_count());
        let wrap_rows = snapshot
            .wrap_snapshot
            .rows(wrap_start, snapshot.wrap_snapshot.line_count());
        let mut this = Self {
            snapshot,
            block_cursor,
            wrap_rows,
            row: start,
            end,
        };
        this.seek_wrap_to_current_transform();
        this
    }

    fn seek_wrap_to_current_transform(&mut self) {
        if self.block_cursor.item().is_some() {
            let wrap_row = self.current_wrap_row();
            if let Some(wrap_row) = wrap_row {
                self.wrap_rows.seek_forward(wrap_row);
            }
        }
    }

    fn current_wrap_row(&self) -> Option<usize> {
        let transform = self.block_cursor.item()?;
        let start = *self.block_cursor.start();
        Some(match &transform.kind {
            TransformKind::Text => start.1.0 + self.row.saturating_sub(start.0.0),
            TransformKind::Block(_) => start.1.0,
        })
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
                let excerpt = self.snapshot.excerpt_for_placement(placement)?;
                let display_row = transform_start.0.0;
                self.row = (display_row + height).min(self.end);
                self.block_cursor.next();
                self.seek_wrap_to_current_transform();
                Some(BlockRow {
                    index: DisplayRow::new(display_row),
                    height,
                    kind: BlockRowKind::Block(DisplayBlock { kind, excerpt }),
                    excerpt: None,
                })
            }
            TransformKind::Text => {
                let wrap_row = transform_start.1.0 + self.row - transform_start.0.0;
                self.wrap_rows.seek_forward(wrap_row);
                let wrap = self.wrap_rows.next()?;
                let excerpt = self.wrap_rows.current_excerpt();
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
fn wrap_span(snapshot: &WrapSnapshot, range: MultiBufferRange) -> Range<usize> {
    let start = snapshot
        .offset_to_wrap_point(range.start())
        .expect("窗口起点必须属于换行快照")
        .row()
        .get();
    let end = if range.end() == snapshot.buffer_snapshot().len_bytes() {
        snapshot.line_count()
    } else {
        snapshot
            .offset_to_wrap_point(range.end())
            .expect("窗口终点必须属于换行快照")
            .row()
            .get()
    };
    start..end
}

/// 只读取变化范围内的逻辑边界；未变化的块描述保留在变换树中。
fn compute_specs_in_range(
    snapshot: &WrapSnapshot,
    folded_buffers: &HashSet<BufferId>,
    rows: Range<usize>,
) -> Vec<BlockSpec> {
    let buffer = snapshot.buffer_snapshot();
    let start = snapshot
        .wrap_point_to_offset_with_bias(
            WrapPoint::new(WrapRow::new(rows.start), DisplayColumn::ZERO),
            FoldBias::Left,
        )
        .expect("块 patch 起点必须属于换行快照");
    let end = if rows.end == snapshot.line_count() {
        buffer.len_bytes()
    } else {
        snapshot
            .wrap_point_to_offset_with_bias(
                WrapPoint::new(WrapRow::new(rows.end), DisplayColumn::ZERO),
                FoldBias::Left,
            )
            .expect("块 patch 终点必须属于换行快照")
    };
    let mut specs = Vec::new();
    for boundary in buffer.excerpt_boundaries_in_range(start..=end) {
        let excerpt = boundary.next();
        let row = snapshot
            .offset_to_wrap_point(excerpt.output_range().start())
            .expect("逻辑边界必须属于换行快照")
            .row()
            .get();
        if row < rows.start || row >= rows.end {
            continue;
        }
        let folded = buffer.show_headers() && folded_buffers.contains(&excerpt.buffer_id());
        if folded && !boundary.starts_new_buffer() {
            continue;
        }
        let Some(kind) = entry_block_kind(
            buffer.show_headers(),
            boundary.next_index() == 0,
            usize::from(!boundary.starts_new_buffer()),
        ) else {
            continue;
        };
        let hidden_end = if folded {
            wrap_span(
                snapshot,
                buffer
                    .buffer_range(excerpt.buffer_id())
                    .expect("边界必须属于当前文件索引"),
            )
            .end
        } else {
            row
        };
        specs.push(BlockSpec {
            wrap_row: row,
            hidden_end,
            placement: Arc::new(BlockPlacement {
                anchor: buffer.anchor_at(excerpt.output_range().start(), Affinity::Before),
                folded,
                height: if kind == DisplayBlockKind::BufferHeader {
                    FILE_HEADER_HEIGHT
                } else {
                    EXCERPT_BOUNDARY_HEIGHT
                },
                kind,
            }),
        });
    }
    specs
}

fn push_text_rows(transforms: &mut SumTree<Transform>, rows: usize) {
    if rows == 0 {
        return;
    }
    if transforms
        .last()
        .is_some_and(|last| last.kind == TransformKind::Text)
    {
        transforms.update_last(
            |last| {
                last.input_rows += rows;
                last.output_rows += rows;
            },
            (),
        );
    } else {
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

fn append_transforms(transforms: &mut SumTree<Transform>, mut suffix: SumTree<Transform>) {
    if transforms
        .last()
        .is_some_and(|last| last.kind == TransformKind::Text)
        && let Some(first) = suffix
            .first()
            .filter(|first| first.kind == TransformKind::Text)
    {
        let rows = first.input_rows;
        let mut cursor = suffix.cursor::<InputRows>(());
        cursor.next();
        cursor.next();
        let remaining = cursor.suffix();
        drop(cursor);
        suffix = remaining;
        push_text_rows(transforms, rows);
    }
    transforms.append(suffix, ());
}

fn materialize_range(
    snapshot: &WrapSnapshot,
    folded: &HashSet<BufferId>,
    rows: Range<usize>,
) -> SumTree<Transform> {
    let mut transforms = SumTree::new(());
    let mut row = rows.start;
    for spec in compute_specs_in_range(snapshot, folded, rows.clone()) {
        assert!(
            spec.wrap_row >= row && spec.hidden_end <= rows.end,
            "块 patch 必须完整覆盖替换块"
        );
        push_text_rows(&mut transforms, spec.wrap_row - row);
        let height = spec.placement.height;
        transforms.push(
            Transform {
                kind: TransformKind::Block(spec.placement),
                input_rows: spec.hidden_end - spec.wrap_row,
                output_rows: height,
            },
            (),
        );
        row = spec.hidden_end;
    }
    push_text_rows(&mut transforms, rows.end - row);
    transforms
}

fn geometry_in_range(
    tree: &SumTree<Transform>,
    rows: Range<usize>,
) -> Vec<(Option<DisplayBlockKind>, usize, usize)> {
    let mut cursor = tree.cursor::<InputRows>(());
    cursor.seek(&InputRows(rows.start), Bias::Left);
    let mut geometry = Vec::new();
    while let Some(transform) = cursor.item() {
        let start = cursor.start().0;
        let end = cursor.end().0;
        if start >= rows.end {
            break;
        }
        match &transform.kind {
            TransformKind::Text => {
                let count = end.min(rows.end).saturating_sub(start.max(rows.start));
                if count > 0 {
                    geometry.push((None, count, count));
                }
            }
            TransformKind::Block(block) if start >= rows.start => geometry.push((
                Some(block.kind),
                transform.input_rows,
                transform.output_rows,
            )),
            TransformKind::Block(_) => {}
        }
        cursor.next();
    }
    geometry
}

impl BlockSnapshot {
    pub(super) fn wrap_snapshot(&self) -> &WrapSnapshot {
        &self.wrap_snapshot
    }

    /// 源到显示行的几何代际；换行断点或逻辑窗口映射变化都必须推进。
    pub(super) fn geometry_epoch(&self) -> u64 {
        self.geometry_epoch
    }

    pub(crate) fn rows(&self, start_row: DisplayRow, line_count: usize) -> BlockRows<'_> {
        BlockRows::new(self, start_row, line_count)
    }

    pub(super) fn point_cursor(&self) -> BlockPointCursor<'_> {
        BlockPointCursor {
            snapshot: self,
            cursor: self.transforms.cursor::<InputToOutput>(()),
        }
    }

    pub(super) fn new(
        wrap_snapshot: WrapSnapshot,
        folded_buffers: &Arc<HashSet<BufferId>>,
    ) -> Self {
        let transforms = materialize_range(
            &wrap_snapshot,
            folded_buffers,
            0..wrap_snapshot.line_count(),
        );
        let show_headers = wrap_snapshot.buffer_snapshot().show_headers();
        let snapshot = Self {
            wrap_snapshot,
            transforms,
            folded_buffers: folded_buffers.clone(),
            show_headers,
            geometry_epoch: 0,
        };
        snapshot.check_invariants();
        snapshot
    }

    pub(super) fn folded_buffers_match(&self, folded: &Arc<HashSet<BufferId>>) -> bool {
        Arc::ptr_eq(&self.folded_buffers, folded)
    }

    pub(super) fn has_expanded_buffers(&self) -> bool {
        self.transforms.summary().expanded_headers > 0
    }

    fn excerpt_for_placement(&self, placement: &BlockPlacement) -> Option<ExcerptSnapshot> {
        let buffer = self.wrap_snapshot.buffer_snapshot();
        let offset = buffer
            .anchor_offset(&placement.anchor)
            .expect("块 Anchor 必须在当前快照上有效");
        buffer.logical_excerpt_at_output_offset(offset)
    }

    /// WrapEdit 与逻辑边界／策略变化合并成同一份块 patch；沿旧树拼接未变化的部分。
    pub(super) fn sync(
        &self,
        wrap_snapshot: WrapSnapshot,
        folded_buffers: &Arc<HashSet<BufferId>>,
        wrap_edits: &[WrapEdit],
    ) -> Self {
        let show_headers = wrap_snapshot.buffer_snapshot().show_headers();
        let mut edits = wrap_edits.to_vec();
        let boundary_change = wrap_snapshot
            .buffer_snapshot()
            .excerpt_boundary_change(self.wrap_snapshot.buffer_snapshot());
        let mut geometry_changed = !wrap_edits.is_empty() || boundary_change.is_some();
        if let Some((old, new)) = boundary_change {
            edits.push(WrapEdit {
                old: wrap_span(&self.wrap_snapshot, old),
                new: wrap_span(&wrap_snapshot, new),
            });
        }
        if !Arc::ptr_eq(&self.folded_buffers, folded_buffers) {
            for id in self.folded_buffers.symmetric_difference(folded_buffers) {
                if let (Some(old), Some(new)) = (
                    self.wrap_snapshot.buffer_snapshot().buffer_range(*id),
                    wrap_snapshot.buffer_snapshot().buffer_range(*id),
                ) {
                    edits.push(WrapEdit {
                        old: wrap_span(&self.wrap_snapshot, old),
                        new: wrap_span(&wrap_snapshot, new),
                    });
                }
            }
        }
        if show_headers != self.show_headers {
            edits.push(WrapEdit {
                old: 0..self.wrap_snapshot.line_count(),
                new: 0..wrap_snapshot.line_count(),
            });
        }
        edits.sort_by_key(|edit| edit.old.start);
        let mut merged: Vec<WrapEdit> = Vec::new();
        for edit in edits {
            if let Some(last) = merged.last_mut()
                && edit.old.start <= last.old.end
            {
                if edit.old.end >= last.old.end {
                    last.old.end = edit.old.end;
                    last.new.end = last.new.end.max(edit.new.end);
                }
                last.new.start = last.new.start.min(edit.new.start);
            } else {
                merged.push(edit);
            }
        }
        if merged.is_empty() {
            let snapshot = Self {
                wrap_snapshot,
                transforms: self.transforms.clone(),
                folded_buffers: folded_buffers.clone(),
                show_headers,
                geometry_epoch: self.geometry_epoch,
            };
            snapshot.check_invariants();
            return snapshot;
        }
        let mut transforms = SumTree::new(());
        let mut cursor = self.transforms.cursor::<InputRows>(());
        let mut edits = merged.into_iter().peekable();
        while let Some(edit) = edits.next() {
            let mut old_start = edit.old.start;
            let mut new_start = edit.new.start;
            append_transforms(
                &mut transforms,
                cursor.slice(&InputRows(old_start), Bias::Left),
            );
            if cursor.item().is_some_and(|transform| {
                transform.kind == TransformKind::Text && cursor.end().0 == old_start
            }) {
                push_text_rows(
                    &mut transforms,
                    cursor.item().expect("文本变换必须存在").input_rows,
                );
                cursor.next();
            }
            if let Some(transform) = cursor.item() {
                let prefix = old_start - cursor.start().0;
                if transform.kind == TransformKind::Text {
                    push_text_rows(&mut transforms, prefix);
                } else if prefix > 0 {
                    old_start -= prefix;
                    new_start -= prefix;
                }
            }
            let mut old_end = edit.old.end;
            let mut new_end = edit.new.end;
            loop {
                cursor.seek(&InputRows(old_end), Bias::Left);
                if cursor.item().is_some() {
                    cursor.next();
                }
                let extra = cursor.start().0 - old_end;
                old_end += extra;
                new_end += extra;
                if let Some(next) = edits.peek()
                    && next.old.start <= old_end
                {
                    let next = edits.next().expect("后续 patch 必须存在");
                    old_end = next.old.end.max(old_end);
                    new_end = next.new.end.max(new_end);
                    continue;
                }
                break;
            }
            let rebuilt = materialize_range(&wrap_snapshot, folded_buffers, new_start..new_end);
            geometry_changed |= geometry_in_range(&self.transforms, old_start..old_end)
                != geometry_in_range(&rebuilt, 0..new_end - new_start);
            append_transforms(&mut transforms, rebuilt);
        }
        append_transforms(&mut transforms, cursor.suffix());
        let snapshot = Self {
            wrap_snapshot,
            transforms,
            folded_buffers: folded_buffers.clone(),
            show_headers,
            geometry_epoch: self.geometry_epoch + u64::from(geometry_changed),
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

    /// 返回视口顶部所在 excerpt 的文件标题。
    ///
    /// 同一文件的后续 excerpt 只有分隔块，但它同样会更新标题所代表的 excerpt，使“打开文件”等操作仍以当前可见片段为目标。
    ///
    /// 与 Zed `BlockSnapshot::sticky_header_excerpt` 相同：以视口顶行为 key 在块变换树上 seek，再回退到最近的块边界，
    /// 成本是 O(log 变换数 + 相邻块数)，不随组合文档规模增长。
    /// 下一个文件标题属于当前帧的可见布局，由渲染层从可见块派生，本查询不向前扫描。
    pub(super) fn sticky_buffer_header(&self, top_row: DisplayRow) -> Option<StickyBufferHeader> {
        let mut cursor = self.transforms.cursor::<OutputToInput>(());
        cursor.seek(&OutputRows(top_row.get()), Bias::Right);
        if cursor.item().is_none() {
            // 视口顶行落在投影末尾之后：从最后一个变换回退。
            cursor.prev();
        }
        loop {
            let transform = cursor.item()?;
            let start_row = cursor.start().0.0;
            if start_row <= top_row.get()
                && let TransformKind::Block(placement) = &transform.kind
            {
                return Some(StickyBufferHeader {
                    source_row: DisplayRow::new(start_row),
                    excerpt: self.excerpt_for_placement(placement)?,
                });
            }
            if start_row == 0 {
                return None;
            }
            cursor.prev();
        }
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
            RowMapping::Block(placement) => self
                .excerpt_for_placement(placement)
                .map(|excerpt| excerpt.output_range().start())
                .ok_or_else(|| {
                    CoordinateError::LineOutOfBounds(Line::new(point.row().get())).into()
                }),
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

    pub(super) fn line_to_display_row(&self, offset: MultiBufferOffset) -> Option<DisplayRow> {
        self.offset_to_display_point(offset)
            .ok()
            .map(DisplayPoint::row)
    }
}

/// 按 Wrap 点顺序映射显示点，复用块插入变换树位置。
pub(crate) struct BlockPointCursor<'a> {
    snapshot: &'a BlockSnapshot,
    cursor: sum_tree::Cursor<'a, 'static, Transform, InputToOutput>,
}

impl BlockPointCursor<'_> {
    pub fn reset(&mut self) {
        self.cursor.reset();
    }

    pub fn map(&mut self, point: WrapPoint) -> DisplayPoint {
        let input_row = point.row().get();
        if self.cursor.did_seek() && input_row >= self.cursor.start().0.0 {
            self.cursor.seek_forward(&InputRows(input_row), Bias::Right);
        } else {
            self.cursor.seek(&InputRows(input_row), Bias::Right);
        }

        let Some(transform) = self.cursor.item() else {
            return DisplayPoint::new(DisplayRow::new(self.snapshot.line_count()), point.column());
        };
        let (output_row, column) = match transform.kind {
            TransformKind::Text => (
                self.cursor.start().1.0 + input_row - self.cursor.start().0.0,
                point.column(),
            ),
            TransformKind::Block(_) => (self.cursor.start().1.0, DisplayColumn::ZERO),
        };
        DisplayPoint::new(DisplayRow::new(output_row), column)
    }
}

#[cfg(test)]
#[path = "test/block_map_tests.rs"]
mod tests;
