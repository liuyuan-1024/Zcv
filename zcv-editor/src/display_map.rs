//! 决定 Buffer 文本如何映射到 Editor 的显示坐标。
//!
//! DisplayMap 由一组自底向上的变换层组成：
//! - FoldMap：维护折叠范围和折叠后的文本拓扑；
//! - TabMap：在 FoldSnapshot 之上处理硬 Tab 的显示列；
//! - WrapMap：在 TabSnapshot 之上按像素宽度软换行。
//! - BlockMap：在换行结果上插入文件标题、片段分隔线等非文本虚拟块。
//!
//! 每一层都持有自己的 Map 和不可变 Snapshot；
//! 上一层 Snapshot 固化下一层 Snapshot，从而让一次渲染只能看到一条内部一致的显示状态。

use zcv_multi_buffer::{MultiBufferAnchor, MultiBufferOffset, MultiBufferRange};

mod block_map;
mod chunk;
mod crease_map;
mod decorations;
mod display_width;
mod edit;
mod error;
mod fold_map;
mod tab_map;
mod wrap_map;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use crate::scrollbar::{ScrollbarMarker, marker_geometry};

use block_map::BlockSnapshot;
pub(crate) use block_map::{
    BlockRows, DisplayBlock, DisplayBlockKind, FILE_HEADER_HEIGHT, StickyBufferHeader,
};
use chunk::MAX_RENDERED_LINE_LEN;
pub(crate) use chunk::{
    BlockChunks as DisplayChunks, DisplayRowEvent, HighlightStyles, RenderedWhitespace,
    WrapRowInfo, chunk_to_run,
};
#[cfg(test)]
pub(crate) use chunk::{ChunkSource, ChunkText, WrapChunks};
pub(crate) use crease_map::CreaseId;
use crease_map::{Crease, CreaseMap, CreaseSnapshot};
#[cfg(test)]
pub(crate) use decorations::hunk_rendering;
pub(crate) use decorations::{
    DiffDecorationSnapshot, DisplayDecorations, SearchDecorationInput, SearchDecorationSnapshot,
    diff_row_for_row, is_hollow_hunk,
};
pub use decorations::{EditorHunk, EditorHunkMarkerKind, EditorHunkPart, HunkControlTarget};
pub(crate) use display_width::DisplayColumn;
use edit::ProjectionEdit;
use error::DisplayMapResult;
pub(crate) use fold_map::{
    ChunkRenderer, FoldBias, FoldPlaceholder, FoldRowSegment, ProjectedLineIndex,
};
use fold_map::{FoldMap, FoldSnapshot};
use gpui::{App, AppContext as _, Bounds, Context, Entity, HighlightStyle, Pixels};
use tab_map::{TabMap, display_width_for_fold_row};
pub(crate) use tab_map::{byte_for_display_column, display_column_for_byte};
use wrap_map::{WrapEdit, WrapMap, WrapSnapshot};
use zcv_language::HighlightSpan;
use zcv_multi_buffer::{
    DiffDisplaySnapshot, MultiBuffer, MultiBufferSnapshot, MultiBufferSubscription,
};
use zcv_text::{
    BufferId, Line, LineRange, LogicalColumn, MovementDirection, MovementUnit, Position,
    TextChangeBatch, TextResult,
};
use zcv_theme::syntax;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub(crate) struct DisplayRow(usize);

impl DisplayRow {
    pub(crate) const ZERO: Self = Self(0);

    pub(crate) const fn new(value: usize) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> usize {
        self.0
    }
}

impl From<ProjectedLineIndex> for DisplayRow {
    fn from(value: ProjectedLineIndex) -> Self {
        Self::new(value.get())
    }
}

/// 软换行层的输出行号（尚未插入文件标题/分隔块）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub(crate) struct WrapRow(usize);

impl WrapRow {
    pub(crate) const fn new(value: usize) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> usize {
        self.0
    }
}

impl From<WrapRow> for DisplayRow {
    fn from(value: WrapRow) -> Self {
        Self::new(value.get())
    }
}

/// 软换行层内的点：换行行号 + 显示列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct WrapPoint {
    row: WrapRow,
    column: DisplayColumn,
}

impl WrapPoint {
    pub(crate) const fn new(row: WrapRow, column: DisplayColumn) -> Self {
        Self { row, column }
    }

    pub(crate) const fn row(self) -> WrapRow {
        self.row
    }

    pub(crate) const fn column(self) -> DisplayColumn {
        self.column
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct DisplayPoint {
    row: DisplayRow,
    column: DisplayColumn,
}

impl DisplayPoint {
    pub(crate) const ZERO: Self = Self {
        row: DisplayRow::ZERO,
        column: DisplayColumn::ZERO,
    };

    pub(crate) const fn new(row: DisplayRow, column: DisplayColumn) -> Self {
        Self { row, column }
    }

    pub(crate) const fn row(self) -> DisplayRow {
        self.row
    }

    pub(crate) const fn column(self) -> DisplayColumn {
        self.column
    }
}

/// 组合文档范围投影后的显示范围（起点、终点均为显示点）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DisplayRange {
    start: DisplayPoint,
    end: DisplayPoint,
}

impl DisplayRange {
    pub(crate) const fn new(start: DisplayPoint, end: DisplayPoint) -> Self {
        Self { start, end }
    }

    pub(crate) const fn start(self) -> DisplayPoint {
        self.start
    }

    pub(crate) const fn end(self) -> DisplayPoint {
        self.end
    }
}

/// 语法折叠候选的按行派生索引；只保留可折叠行，随显示版本整体替换。
///
/// 派生集中在消费端首次看到某段视口时发生，之后滚动帧只做区间查表。
#[derive(Debug, Default)]
struct SyntaxCreaseIndex {
    /// 已派生的连续逻辑行区间；`None` 表示尚未派生。
    covered: Option<Range<Line>>,
    /// 已派生区间内的可折叠行及其候选。
    by_line: BTreeMap<Line, Crease>,
}

impl SyntaxCreaseIndex {
    /// 补齐 `range` 尚未派生的部分；
    /// 与已派生区间不相交时重建，避免为巨大间隙派生。
    fn cover(&mut self, range: Range<Line>, mut crease_at: impl FnMut(Line) -> Option<Crease>) {
        let Some(covered) = self.covered.clone() else {
            self.populate(range.clone(), &mut crease_at);
            self.covered = Some(range);
            return;
        };
        if covered.start <= range.start && range.end <= covered.end {
            return;
        }
        if range.end < covered.start || covered.end < range.start {
            self.by_line.clear();
            self.populate(range.clone(), &mut crease_at);
            self.covered = Some(range);
            return;
        }
        if range.start < covered.start {
            self.populate(range.start..covered.start, &mut crease_at);
        }
        if covered.end < range.end {
            self.populate(covered.end..range.end, &mut crease_at);
        }
        self.covered = Some(covered.start.min(range.start)..covered.end.max(range.end));
    }

    fn populate(&mut self, range: Range<Line>, crease_at: &mut impl FnMut(Line) -> Option<Crease>) {
        for index in range.start.get()..range.end.get() {
            let line = Line::new(index);
            if let Some(crease) = crease_at(line) {
                self.by_line.insert(line, crease);
            }
        }
    }
}

/// 一帧渲染使用的只读显示快照。
///
/// FoldSnapshot、TabSnapshot 与 WrapSnapshot 都是低成本克隆；渲染持有此值时
/// 不会阻塞 Editor 接收后续 Buffer 更新。主题样式不属于显示快照，渲染需要时按当前主题派生。
#[derive(Debug, Clone)]
pub(super) struct DisplaySnapshot {
    /// 唯一的显示拓扑权威；链叶即唯一的 MultiBufferSnapshot。
    block_snapshot: Arc<BlockSnapshot>,
    /// 与本显示版本绑定的折叠候选索引；随快照整体替换、可丢弃。
    crease_snapshot: CreaseSnapshot,
    /// 与本显示版本绑定的显示装饰投影；随快照整体替换、可丢弃。
    decorations: Arc<DisplayDecorations>,
    /// diff 显示输入归组合文档所有；这里只持有当前显示版本的不可变引用。
    diff_display: Option<Arc<DiffDisplaySnapshot>>,
    /// 语法折叠候选的区间派生索引：同一显示版本内只对视口区间派生，随快照整体替换。
    syntax_crease_cache: Arc<Mutex<SyntaxCreaseIndex>>,
    /// 显示版本；每次替换当前显示快照都会前进，后台派生结果据此判断是否过期。
    version: u64,
}

impl DisplaySnapshot {
    /// diff hunk 装饰的视口投影。
    pub(crate) fn diff_decorations(&self) -> Arc<DiffDecorationSnapshot> {
        self.decorations.diff()
    }

    /// 搜索命中装饰；无搜索时为空。
    pub(crate) fn search_decorations(&self) -> Option<Arc<SearchDecorationSnapshot>> {
        self.decorations.search()
    }

    /// 滚动条慢标记的轨道几何；由 Editor 在后台按显示版本计算并缓存。
    ///
    /// 组合文档只计算 diff 标记，搜索命中标记仅单文档编辑器计算（对齐 Zed）。
    pub(crate) fn scrollbar_marker_groups(
        &self,
        track_bounds: Bounds<Pixels>,
        scroll_per_pixel: f32,
        line_height: Pixels,
        is_singleton: bool,
    ) -> [Option<Arc<[ScrollbarMarker]>>; 2] {
        let diff_markers = Arc::from(
            marker_geometry(
                self.diff_decorations().scrollbar_marker_ranges(),
                track_bounds,
                scroll_per_pixel,
                line_height,
            )
            .into_boxed_slice(),
        );
        let search_markers = is_singleton
            .then(|| {
                self.search_decorations().map(|search| {
                    Arc::from(
                        marker_geometry(
                            search.scrollbar_marker_ranges(self),
                            track_bounds,
                            scroll_per_pixel,
                            line_height,
                        )
                        .into_boxed_slice(),
                    )
                })
            })
            .flatten();
        [Some(diff_markers), search_markers]
    }

    /// 返回指定逻辑行的折叠候选。
    ///
    /// 显式 crease 始终实时查询（可能被宿主增删）；
    /// 语法候选按显示版本派生到行索引，同一区间只派生一次，滚动帧只做区间查表，不在渲染路径逐行重跑源投影。
    pub(crate) fn crease_at_line(&self, line: Line) -> Option<Crease> {
        self.crease_snapshot
            .crease_at_line(line, self.buffer_snapshot())
            .cloned()
            .or_else(|| self.syntax_crease_at_line_cached(line))
    }

    /// 视口范围内可折叠的逻辑行；只对视口区间派生一次。
    pub(super) fn foldable_lines_in_range(&self, range: Range<Line>) -> BTreeSet<Line> {
        let mut index = self
            .syntax_crease_cache
            .lock()
            .expect("语法折叠候选派生缓存锁不得中毒");
        index.cover(range.clone(), |line| self.syntax_crease_at_line(line));
        index.by_line.range(range).map(|(line, _)| *line).collect()
    }

    fn syntax_crease_at_line_cached(&self, line: Line) -> Option<Crease> {
        let mut index = self
            .syntax_crease_cache
            .lock()
            .expect("语法折叠候选派生缓存锁不得中毒");
        index.cover(Line::new(line.get())..Line::new(line.get() + 1), |line| {
            self.syntax_crease_at_line(line)
        });
        index.by_line.get(&line).cloned()
    }

    /// 返回包含指定逻辑行的最内层折叠候选，供光标位于折叠体内部时的切换命令使用。
    pub(crate) fn crease_containing_line(&self, line: Line) -> Option<Crease> {
        self.explicit_crease_containing_line(line)
            .or_else(|| self.syntax_crease_containing_line(line))
    }

    fn explicit_crease_containing_line(&self, line: Line) -> Option<Crease> {
        self.crease_snapshot
            .creases()
            .filter_map(|crease| {
                let range = crease.range();
                let start = self
                    .buffer_snapshot()
                    .resolve_anchor(&range.start)
                    .and_then(|offset| self.buffer_snapshot().byte_to_line(offset).ok())?;
                let end = self
                    .buffer_snapshot()
                    .resolve_anchor(&range.end)
                    .and_then(|offset| self.buffer_snapshot().byte_to_line(offset).ok())?;
                (start <= line && line <= end).then_some((crease, start, end))
            })
            .min_by_key(|(_, start, end)| (line.get() - start.get(), end.get() - line.get()))
            .map(|(crease, _, _)| crease.clone())
    }

    fn syntax_crease_at_line(&self, line: Line) -> Option<Crease> {
        let buffer = self.buffer_snapshot();
        let offset = buffer.line_start_byte(line).ok()?;
        let source = buffer.source_at(offset)?;
        let source_text = source.text();
        let source_line = source_text.byte_to_line(source.source_offset()).ok()?;
        let source_line_start = source_text.line_start_byte(source_line).ok()?;
        let source_line_end = if source_line.get() + 1 < source_text.line_count() {
            source_text
                .line_start_byte(Line::new(source_line.get() + 1))
                .ok()?
        } else {
            source_text.len_bytes()
        };

        source
            .syntax()
            .fold_ranges(source_line_start.get()..source_line_end.get(), source_text)
            .into_iter()
            .filter_map(|fold| {
                let start = fold.range.start.resolve_in(source_text).ok()?;
                let end = fold.range.end.resolve_in(source_text).ok()?;
                source.project_range(start..end)
            })
            .filter(|range| {
                buffer
                    .resolve_anchor(&range.start)
                    .and_then(|offset| buffer.byte_to_line(offset).ok())
                    == Some(line)
            })
            .min_by_key(|range| {
                buffer
                    .resolve_anchor(&range.end)
                    .map_or(usize::MAX, |end| end.get())
            })
            .map(Crease::simple)
    }

    fn syntax_crease_containing_line(&self, line: Line) -> Option<Crease> {
        let buffer = self.buffer_snapshot();
        let offset = buffer.line_start_byte(line).ok()?;
        let source = buffer.source_at(offset)?;
        let source_text = source.text();

        source
            .syntax()
            .fold_ranges(0..source_text.len_bytes().get(), source_text)
            .into_iter()
            .filter_map(|fold| {
                let start = fold.range.start.resolve_in(source_text).ok()?;
                let end = fold.range.end.resolve_in(source_text).ok()?;
                source.project_range(start..end)
            })
            .filter_map(|range| {
                let start = buffer
                    .resolve_anchor(&range.start)
                    .and_then(|offset| buffer.byte_to_line(offset).ok())?;
                let end = buffer
                    .resolve_anchor(&range.end)
                    .and_then(|offset| buffer.byte_to_line(offset).ok())?;
                (start <= line && line <= end).then_some((range, start, end))
            })
            .min_by_key(|(_, start, end)| (line.get() - start.get(), end.get() - line.get()))
            .map(|(range, _, _)| Crease::simple(range))
    }

    pub(super) fn tab_width(&self) -> NonZeroUsize {
        self.wrap_snapshot().tab_snapshot().tab_width()
    }

    pub(super) fn wrap_snapshot(&self) -> &WrapSnapshot {
        self.block_snapshot.wrap_snapshot()
    }

    pub(super) fn buffer_snapshot(&self) -> &MultiBufferSnapshot {
        self.wrap_snapshot().buffer_snapshot()
    }

    fn fold_snapshot(&self) -> &FoldSnapshot {
        self.wrap_snapshot().tab_snapshot().fold_snapshot()
    }

    /// 从连续源行范围消费高亮。布局使用显示游标先得到该范围，再由这里一次查询语法 chunk 流。
    pub(super) fn highlighted_spans_for_source_ranges(
        &self,
        line_ranges: impl IntoIterator<Item = Range<Line>>,
    ) -> Arc<[HighlightSpan]> {
        let buffer = self.buffer_snapshot();
        let mut byte_ranges = Vec::new();
        for line_range in line_ranges {
            let Ok(start) = buffer.line_start_byte(line_range.start) else {
                continue;
            };
            let end = if line_range.end.get() < buffer.line_count() {
                match buffer.line_start_byte(line_range.end) {
                    Ok(end) => end,
                    Err(_) => continue,
                }
            } else {
                buffer.len_bytes()
            };
            let mut end = end.get();
            if line_range.end.get() == line_range.start.get().saturating_add(1)
                && end.saturating_sub(start.get()) > MAX_RENDERED_LINE_LEN
            {
                end = start.get() + MAX_RENDERED_LINE_LEN;
            }
            byte_ranges.push(start.get()..end);
        }
        self.highlighted_spans_for_ranges(byte_ranges)
    }

    fn highlighted_spans_for_ranges(&self, ranges: Vec<Range<usize>>) -> Arc<[HighlightSpan]> {
        let mut spans = Vec::new();
        for range in &ranges {
            spans.extend(self.buffer_snapshot().highlights(range.clone()));
        }
        Arc::from(spans)
    }

    /// 按当前 App 主题生成 capture 索引 → 样式的预展开表。
    pub(super) fn highlight_styles(&self, cx: &App) -> Vec<HighlightStyle> {
        syntax::style_table(&self.buffer_snapshot().capture_names(), cx)
    }

    /// 当前显示快照的版本；每次替换显示快照都会前进。
    pub(super) fn version(&self) -> u64 {
        self.version
    }

    pub(super) fn line_count(&self) -> usize {
        self.block_snapshot.line_count()
    }

    /// 折叠入口行集合（crease 折叠态与占位符命中判断）。
    pub(super) fn fold_anchor_lines(&self) -> Vec<Line> {
        self.fold_snapshot().fold_anchor_lines()
    }

    pub(super) fn fold_anchor_lines_in_range(&self, line_range: Range<Line>) -> Vec<Line> {
        self.fold_snapshot().fold_anchor_lines_in_range(line_range)
    }

    /// 覆盖该字节偏移的最外层折叠的隐藏范围（水平移动跨折叠吸附用）。
    pub(super) fn fold_range_covering_offset(
        &self,
        offset: MultiBufferOffset,
    ) -> Option<(MultiBufferOffset, MultiBufferOffset)> {
        self.fold_snapshot().fold_range_covering_offset(offset)
    }

    /// 逻辑行 → 该行首个显示行（wrap 下行首）；行号越界返回 None。
    ///
    /// 组合 position_to_byte + offset_to_display_point（滚动轴 diff marker 用）。
    pub(super) fn line_to_display_row(&self, line: Line) -> Option<DisplayRow> {
        let offset = self
            .buffer_snapshot()
            .position_to_byte(Position::new(line, LogicalColumn::ZERO))
            .ok()?;
        self.block_snapshot.line_to_display_row(offset)
    }

    pub(super) fn is_wrapped(&self) -> bool {
        self.wrap_snapshot().is_wrapped()
    }

    pub(crate) fn rows(&self, start_row: DisplayRow, line_count: usize) -> BlockRows<'_> {
        self.block_snapshot.rows(start_row, line_count)
    }

    /// 从显示快照的起点连续消费 Block/Fold/Wrap 产生的 chunk。
    ///
    /// 这是渲染、宽度测量和命中测试共享的唯一文本消费入口；
    /// 调用方只提供显示行范围，各投影层在快照内部通过持久游标向前推进。
    pub(crate) fn chunks<'a>(
        &self,
        display_rows: Range<DisplayRow>,
        styles: HighlightStyles<'a>,
        window_columns: Option<(usize, usize)>,
    ) -> DisplayChunks<'_, 'a> {
        DisplayChunks::new(self, display_rows, styles, window_columns)
    }

    pub(super) fn project_text_range(
        &self,
        range: MultiBufferRange,
    ) -> DisplayMapResult<Vec<DisplayRange>> {
        self.block_snapshot.project_text_range(range)
    }

    pub(super) fn offset_to_display_point(
        &self,
        offset: MultiBufferOffset,
    ) -> DisplayMapResult<DisplayPoint> {
        self.block_snapshot.offset_to_display_point(offset)
    }

    pub(super) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<MultiBufferOffset> {
        self.block_snapshot.display_point_to_offset(point)
    }

    pub(super) fn display_point_to_offset_with_bias(
        &self,
        point: DisplayPoint,
        bias: FoldBias,
    ) -> DisplayMapResult<MultiBufferOffset> {
        self.block_snapshot
            .display_point_to_offset_with_bias(point, bias)
    }

    pub(super) fn sticky_buffer_header(
        &self,
        top_row: DisplayRow,
    ) -> Option<block_map::StickyBufferHeader> {
        self.block_snapshot.sticky_buffer_header(top_row)
    }

    /// 语法查询读取组合快照的选区扩张范围。
    pub(super) fn ancestor_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        self.buffer_snapshot().expand_selection_range(range)
    }

    /// 查询指定组合文档范围的语法高亮，并解析为当前编辑器主题样式。
    pub(super) fn highlights_for_range(
        &self,
        range: Range<usize>,
        cx: &App,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let highlight_styles = syntax::style_table(&self.buffer_snapshot().capture_names(), cx);
        self.buffer_snapshot()
            .highlights(range)
            .into_iter()
            .filter_map(|span| {
                highlight_styles
                    .get(span.capture as usize)
                    .cloned()
                    .map(|style| (span.range, style))
            })
            .collect()
    }

    /// 按文本移动粒度计算水平目标，并由显示层跨过占位符。
    pub(super) fn move_offset(
        &self,
        offset: MultiBufferOffset,
        direction: MovementDirection,
        unit: MovementUnit,
    ) -> TextResult<MultiBufferOffset> {
        let snapshot = self.buffer_snapshot();
        let char_offset = snapshot.byte_to_char(offset)?;
        let target = snapshot.movement_boundary(char_offset, direction, unit)?;
        let target = snapshot.char_to_byte(target)?;
        Ok(match self.fold_range_covering_offset(target) {
            Some((start, end)) => match direction {
                MovementDirection::Previous => start,
                MovementDirection::Next => end,
            },
            None => target,
        })
    }

    pub(super) fn beginning_of_row(
        &self,
        offset: MultiBufferOffset,
    ) -> DisplayMapResult<MultiBufferOffset> {
        self.wrap_snapshot().beginning_of_row(offset)
    }

    pub(super) fn end_of_row(
        &self,
        offset: MultiBufferOffset,
    ) -> DisplayMapResult<MultiBufferOffset> {
        self.wrap_snapshot().end_of_row(offset)
    }
}

/// 把订阅者独立积累的组合文本批次换算成 Buffer 坐标的投影编辑。
///
/// `DisplayMap` 是唯一文本变更消费者：它把批次换算成组合文本编辑后交给 FoldMap，
/// 其后各层只消费上层编辑，不再回读文本层的 PositionMap。
///
/// 与 Zed 一样，显示层只按实际 `patch` 编辑同步：重载的文本差异保持增量，真正的整篇替换必须由生产方发布覆盖全文的编辑。
pub(crate) fn buffer_edits_from_batch(
    batch: &TextChangeBatch,
) -> Vec<ProjectionEdit<MultiBufferOffset>> {
    batch
        .patch()
        .edits()
        .iter()
        .map(|edit| {
            ProjectionEdit::new(
                MultiBufferOffset::new(edit.old_range().start().get())
                    ..MultiBufferOffset::new(edit.old_range().end().get()),
                MultiBufferOffset::new(edit.new_range().start().get())
                    ..MultiBufferOffset::new(edit.new_range().end().get()),
            )
        })
        .collect()
}

#[derive(Debug)]
pub(crate) struct DisplayMap {
    fold_map: FoldMap,
    tab_map: TabMap,
    /// 换行层实体：它自己拥有配置、变换树、待处理批次与后台重排任务。
    wrap_map: Entity<WrapMap>,
    /// 由 BufferHeader 控制的整文件折叠；BlockMap 在 WrapMap 之上隐藏对应文本行。
    folded_buffers: HashSet<BufferId>,
    /// 当前显示管线的持久派生快照；滚动和普通重绘只克隆快照，不重建 BlockSnapshot。
    snapshot: Option<DisplaySnapshot>,
    /// 组合文本源：DisplayMap 是组合文本变更与同步的唯一持有者。
    multi_buffer: Option<Entity<MultiBuffer>>,
    buffer_subscription: Option<MultiBufferSubscription>,
    /// 宿主注入的 hunk 显示输入（显示装饰领域键之一）。
    editor_hunks: Arc<[EditorHunk]>,
    /// 搜索命中的显示输入（显示装饰领域键之一）。
    search: Option<SearchDecorationInput>,
    /// 宿主注入的显式折叠候选；语法候选由 `DisplaySnapshot` 按行即时查询。
    crease_map: CreaseMap,
    /// 当前显示快照的版本号；每次替换 `snapshot` 时前进。
    display_version: u64,
}

fn default_tab_width() -> NonZeroUsize {
    NonZeroUsize::new(4).expect("默认 tab 宽度必须大于 0")
}

impl DisplayMap {
    /// 前进显示版本；任何替换当前显示快照的路径都必须调用它。
    fn next_display_version(&mut self) -> u64 {
        self.display_version += 1;
        self.display_version
    }
}

/// 两份搜索装饰输入是否等价：范围句柄相同且活动序号相同。
///
/// 命中范围由 `Editor` 持有并在未变化时复用同一 `Arc`；
/// 这里用身份比较避免每次推进都逐项比较整份命中。
fn search_input_eq(a: Option<&SearchDecorationInput>, b: Option<&SearchDecorationInput>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.active_index == b.active_index && Arc::ptr_eq(&a.ranges, &b.ranges),
        _ => false,
    }
}

fn same_diff_display(
    current: Option<&Arc<DiffDisplaySnapshot>>,
    next: Option<&Arc<DiffDisplaySnapshot>>,
) -> bool {
    match (current, next) {
        (None, None) => true,
        (Some(current), Some(next)) => Arc::ptr_eq(current, next),
        _ => false,
    }
}

impl DisplayMap {
    pub(crate) fn new(snapshot: impl Into<MultiBufferSnapshot>, cx: &mut Context<Self>) -> Self {
        let snapshot = snapshot.into();
        let tab_width = default_tab_width();
        let (fold_map, fold_snapshot) = FoldMap::new(snapshot.clone());
        let (tab_map, tab_snapshot) = TabMap::new(fold_snapshot, tab_width);
        let wrap_map = cx.new(|_| WrapMap::new(tab_snapshot));
        let wrap_snapshot = wrap_map.read(cx).snapshot().clone();
        let mut this = Self {
            fold_map,
            tab_map,
            wrap_map,
            folded_buffers: HashSet::new(),
            snapshot: None,
            multi_buffer: None,
            buffer_subscription: None,
            editor_hunks: Arc::from([]),
            search: None,
            crease_map: CreaseMap::new(&snapshot),
            display_version: 0,
        };
        this.commit_snapshot(&wrap_snapshot, &[], cx);
        // 换行层自己拥有后台重排；
        // 完成后只唤醒统一读取入口，Block 投影推进仍由 DisplayMap::snapshot 的 sync 完成。
        cx.observe(&this.wrap_map, |_, _, cx| cx.notify()).detach();
        this
    }

    /// 绑定组合文档；显示投影只在读取快照时消费组合订阅。
    pub(crate) fn set_multi_buffer(
        &mut self,
        multi_buffer: Entity<MultiBuffer>,
        subscription: MultiBufferSubscription,
        cx: &mut Context<Self>,
    ) {
        self.multi_buffer = Some(multi_buffer);
        self.buffer_subscription = Some(subscription);
        // 绑定后重建一次装饰，使 diff 输入立即可见。
        let wrap_snapshot = self.wrap_map.read(cx).snapshot().clone();
        self.commit_snapshot(&wrap_snapshot, &[], cx);
    }

    /// 读取并推进当前显示快照；组合文本同步、换行与块投影都从这里进入。
    /// 折叠候选按显示版本在消费端派生缓存，不参与显示拓扑同步。
    pub(crate) fn snapshot(&mut self, cx: &mut Context<Self>) -> DisplaySnapshot {
        let Some(multi_buffer) = self.multi_buffer.clone() else {
            return self.cached_snapshot();
        };
        let snapshot = multi_buffer.update(cx, |buffer, cx| buffer.snapshot(cx));
        let changes = self
            .buffer_subscription
            .as_ref()
            .map_or_else(TextChangeBatch::default, |subscription| {
                subscription.consume()
            });
        self.sync(snapshot, changes, cx);
        self.snapshot
            .as_ref()
            .expect("DisplayMap 同步后必须存在显示快照")
            .clone()
    }

    /// 返回最近一次已同步的显示快照；不会触发组合文档同步。
    pub(crate) fn cached_snapshot(&self) -> DisplaySnapshot {
        self.snapshot
            .as_ref()
            .expect("DisplayMap 初始化后必须存在显示快照")
            .clone()
    }

    /// 宿主注入的 hunk 显示输入；只替换显示链上的装饰，不影响显示拓扑。
    pub(crate) fn set_editor_hunks(&mut self, hunks: Arc<[EditorHunk]>, cx: &mut Context<Self>) {
        if self.editor_hunks == hunks {
            return;
        }
        self.editor_hunks = hunks;
        self.refresh_editor_hunk_decorations(cx);
    }

    /// 替换搜索命中的显示输入；范围已由 Editor 解析到组合坐标。
    pub(crate) fn set_search_decorations(
        &mut self,
        search: Option<SearchDecorationInput>,
        cx: &mut Context<Self>,
    ) {
        if search_input_eq(self.search.as_ref(), search.as_ref()) {
            return;
        }
        self.search = search;
        self.refresh_search_decorations(cx);
    }

    /// 注入宿主拥有的显式折叠候选，并返回其稳定身份。
    ///
    /// 显式范围通过组合锚点保存，会在后续快照上自行解析；语法折叠不进入本索引。
    pub(crate) fn insert_creases(
        &mut self,
        ranges: impl IntoIterator<Item = Range<MultiBufferAnchor>>,
        cx: &mut Context<Self>,
    ) -> Vec<CreaseId> {
        let buffer_snapshot = self.fold_map.snapshot().buffer_snapshot().clone();
        let ids = self
            .crease_map
            .insert(ranges.into_iter().map(Crease::simple), &buffer_snapshot);
        if !ids.is_empty() {
            self.refresh_crease_snapshot(cx);
        }
        ids
    }

    /// 移除由 `insert_creases` 返回身份标识的显式折叠候选。
    pub(crate) fn remove_creases(
        &mut self,
        ids: impl IntoIterator<Item = CreaseId>,
        cx: &mut Context<Self>,
    ) {
        let ids = ids.into_iter().collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        let buffer_snapshot = self.fold_map.snapshot().buffer_snapshot().clone();
        self.crease_map.remove(ids, &buffer_snapshot);
        self.refresh_crease_snapshot(cx);
    }

    fn refresh_crease_snapshot(&mut self, cx: &mut Context<Self>) {
        let Some(mut snapshot) = self.snapshot.take() else {
            return;
        };
        snapshot.crease_snapshot = self.crease_map.snapshot();
        snapshot.version = self.next_display_version();
        self.snapshot = Some(snapshot);
        cx.notify();
    }

    /// 只替换宿主 hunk 影响的 diff 域装饰；搜索域与显示拓扑保持不变。
    fn refresh_editor_hunk_decorations(&mut self, cx: &mut Context<Self>) {
        let Some(mut snapshot) = self.snapshot.take() else {
            return;
        };
        let diff = Arc::new(DiffDecorationSnapshot::new(
            &snapshot,
            snapshot.diff_display.as_deref(),
            &self.editor_hunks,
        ));
        snapshot.decorations = Arc::new(snapshot.decorations.with_diff(diff));
        snapshot.version = self.next_display_version();
        self.snapshot = Some(snapshot);
        cx.notify();
    }

    /// 只替换搜索域装饰；diff 域与显示拓扑保持不变。
    fn refresh_search_decorations(&mut self, cx: &mut Context<Self>) {
        let Some(mut snapshot) = self.snapshot.take() else {
            return;
        };
        let search = self.search.as_ref().map(|input| {
            Arc::new(SearchDecorationSnapshot::from_ranges(
                Arc::clone(&input.ranges),
                input.active_index,
            ))
        });
        snapshot.decorations = Arc::new(snapshot.decorations.with_search(search));
        snapshot.version = self.next_display_version();
        self.snapshot = Some(snapshot);
        cx.notify();
    }

    /// 按当前显示拓扑和装饰输入投影出一份完整装饰。
    fn build_decorations(
        &self,
        snapshot: &DisplaySnapshot,
        cached_diff: Option<Arc<DiffDecorationSnapshot>>,
        _cx: &App,
    ) -> DisplayDecorations {
        DisplayDecorations::new(
            snapshot,
            snapshot.diff_display.as_deref(),
            self.search.as_ref(),
            Arc::clone(&self.editor_hunks),
            cached_diff,
        )
    }

    fn commit_snapshot(&mut self, wrap_snapshot: &WrapSnapshot, wrap_edits: &[WrapEdit], cx: &App) {
        let block_snapshot = Arc::new(self.current_block_snapshot(wrap_snapshot, wrap_edits));
        let version = self.next_display_version();
        let mut snapshot = DisplaySnapshot {
            block_snapshot,
            crease_snapshot: self.crease_map.snapshot(),
            decorations: Arc::new(DisplayDecorations::empty()),
            diff_display: wrap_snapshot.buffer_snapshot().diff_display().cloned(),
            syntax_crease_cache: Arc::new(Mutex::new(SyntaxCreaseIndex::default())),
            version,
        };
        // 复用已投影的 diff 装饰要求显示几何未变：块几何代际是唯一判据。
        // 不能拿「是否有换行重排」代理——折叠、显示策略或 excerpt 拓扑变化同样改变显示行。
        let geometry_unchanged = self.snapshot.as_ref().is_some_and(|previous| {
            previous.block_snapshot.geometry_epoch() == snapshot.block_snapshot.geometry_epoch()
        });
        let cached_diff = self.snapshot.as_ref().and_then(|previous| {
            (geometry_unchanged
                && self.editor_hunks.is_empty()
                && previous.tab_width() == wrap_snapshot.tab_snapshot().tab_width()
                && same_diff_display(
                    previous.diff_display.as_ref(),
                    snapshot.diff_display.as_ref(),
                ))
            .then(|| previous.decorations.diff())
        });
        snapshot.decorations = Arc::new(self.build_decorations(&snapshot, cached_diff, cx));
        self.snapshot = Some(snapshot);
    }

    /// 设置 tab 视觉列宽；变化时重建 tab 与 wrap 投影并刷新当前显示快照。
    pub(crate) fn set_tab_width(&mut self, tab_width: NonZeroUsize, cx: &mut Context<Self>) {
        if self.tab_map.snapshot().tab_width() == tab_width {
            return;
        }
        let fold_snapshot = self.fold_map.snapshot().clone();
        let (tab_snapshot, tab_edits) = self.tab_map.sync(fold_snapshot, &[], tab_width);
        let (wrap_snapshot, wrap_edits) = self
            .wrap_map
            .update(cx, |map, cx| map.sync(tab_snapshot, &tab_edits, cx));
        self.commit_snapshot(&wrap_snapshot, &wrap_edits, cx);
    }

    pub(crate) fn is_buffer_folded(&self, buffer_id: BufferId) -> bool {
        self.folded_buffers.contains(&buffer_id)
    }

    pub(crate) fn set_buffer_folded(
        &mut self,
        buffer_id: BufferId,
        folded: bool,
        cx: &mut Context<Self>,
    ) {
        let changed = if folded {
            self.folded_buffers.insert(buffer_id)
        } else {
            self.folded_buffers.remove(&buffer_id)
        };
        if changed {
            // 折叠是块层策略变化，不是换行重排：不
            // 合成 WrapEdit，由 BlockSnapshot::sync 依据 folded_buffers 重算并推进块几何代际。
            let wrap_snapshot = self.wrap_map.read(cx).snapshot().clone();
            self.commit_snapshot(&wrap_snapshot, &[], cx);
        }
    }

    /// 设置软换行宽度与字体；宽度/字体变化时内部重建，返回是否发生变化。
    pub(crate) fn set_wrap_width(
        &mut self,
        wrap_width: Option<gpui::Pixels>,
        font: gpui::Font,
        font_size: gpui::Pixels,
        text_system: &std::sync::Arc<gpui::TextSystem>,
        cx: &mut Context<Self>,
    ) -> bool {
        let (changed, wrap_edits) = self.wrap_map.update(cx, |map, _cx| {
            map.set_wrap_width(wrap_width, font, font_size, text_system.clone())
        });
        if changed {
            let wrap_snapshot = self.wrap_map.read(cx).snapshot().clone();
            self.commit_snapshot(&wrap_snapshot, &wrap_edits, cx);
        }
        changed
    }

    /// 未开启软换行时按当前 Tab 快照即时计算最长行。
    ///
    /// 该计算不在 TabMap 保存逐行宽度缓存；
    /// Tab 展开仍只在读取当前快照时发生。
    pub(crate) fn longest_unwrapped_row(&self) -> DisplayRow {
        let tab_snapshot = self.tab_map.snapshot();
        let row = (0..tab_snapshot.line_count())
            .max_by_key(|row| {
                display_width_for_fold_row(tab_snapshot, Line::new(*row)).unwrap_or_default()
            })
            .unwrap_or_default();
        self.snapshot
            .as_ref()
            .expect("DisplayMap 初始化后必须存在显示快照")
            .block_snapshot
            .projected_wrap_row_to_display_row(row)
    }

    /// 用订阅者独立积累的组合 Patch，把整条显示管线推进到当前 Snapshot。
    ///
    /// 换行层拥有自己的后台重排；这里消费它本次发布的换行编辑并重建 Block 投影。
    fn sync(
        &mut self,
        current_snapshot: impl Into<MultiBufferSnapshot>,
        batch: TextChangeBatch,
        cx: &mut Context<Self>,
    ) {
        let current_snapshot = current_snapshot.into();
        // 同步始终逐层推进（对齐 Zed DisplayMap::sync_through_wrap）：
        // 无变化的批次由 WrapMap 丢弃、Block 层复用变换树，不在这里做提前返回。
        let buffer_edits = buffer_edits_from_batch(&batch);
        let (fold_snapshot, fold_edits) = self.fold_map.read(current_snapshot, buffer_edits);
        let tab_width = self.tab_map.snapshot().tab_width();
        let (tab_snapshot, tab_edits) = self.tab_map.sync(fold_snapshot, &fold_edits, tab_width);
        let (wrap_snapshot, wrap_edits) = self
            .wrap_map
            .update(cx, |map, cx| map.sync(tab_snapshot, &tab_edits, cx));
        self.commit_snapshot(&wrap_snapshot, &wrap_edits, cx);
    }

    /// 折叠组合锚点范围（入口行行尾换行符 → 闭合括号前；闭合括号保留可见）。
    ///
    /// 端点以组合锚点保存，折叠拓扑在每次同步时按当前快照重新解析。
    pub(crate) fn fold_range(
        &mut self,
        range: Range<MultiBufferAnchor>,
        placeholder: FoldPlaceholder,
        cx: &mut Context<Self>,
    ) -> DisplayMapResult<()> {
        let (fold_snapshot, fold_edits) = self.fold_map.write().fold(range, placeholder)?;
        let tab_width = self.tab_map.snapshot().tab_width();
        let (tab_snapshot, tab_edits) = self.tab_map.sync(fold_snapshot, &fold_edits, tab_width);
        let (wrap_snapshot, wrap_edits) = self
            .wrap_map
            .update(cx, |map, cx| map.sync(tab_snapshot, &tab_edits, cx));
        self.commit_snapshot(&wrap_snapshot, &wrap_edits, cx);
        Ok(())
    }

    /// 展开与行范围交叠的全部折叠（半开区间）。
    pub(crate) fn unfold_lines(
        &mut self,
        line_range: LineRange,
        cx: &mut Context<Self>,
    ) -> DisplayMapResult<()> {
        let (fold_snapshot, fold_edits) = self.fold_map.write().unfold_lines(line_range)?;
        let tab_width = self.tab_map.snapshot().tab_width();
        let (tab_snapshot, tab_edits) = self.tab_map.sync(fold_snapshot, &fold_edits, tab_width);
        let (wrap_snapshot, wrap_edits) = self
            .wrap_map
            .update(cx, |map, cx| map.sync(tab_snapshot, &tab_edits, cx));
        self.commit_snapshot(&wrap_snapshot, &wrap_edits, cx);
        Ok(())
    }

    fn current_block_snapshot(
        &self,
        wrap_snapshot: &WrapSnapshot,
        wrap_edits: &[WrapEdit],
    ) -> BlockSnapshot {
        let excerpts = self.fold_map.snapshot().buffer_snapshot().excerpts_arc();
        // 消费换行编辑流：块布局未变时复用，几何变化时按显式分支重建。
        match &self.snapshot {
            Some(previous) => previous.block_snapshot.sync(
                wrap_snapshot.clone(),
                excerpts,
                &self.folded_buffers,
                wrap_edits,
            ),
            None => BlockSnapshot::new(wrap_snapshot.clone(), excerpts, &self.folded_buffers),
        }
    }
}

#[cfg(test)]
#[path = "display_map/test/support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "test/display_map_tests.rs"]
mod tests;
