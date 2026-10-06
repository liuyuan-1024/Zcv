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

use zcv_multi_buffer::{
    MultiBufferAnchor, MultiBufferOffset, MultiBufferPositionCursor, MultiBufferRange,
};

mod block_map;
mod chunk;
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

use block_map::{BlockPointCursor, BlockSnapshot};
pub(crate) use block_map::{
    BlockRows, DisplayBlock, DisplayBlockKind, FILE_HEADER_HEIGHT, StickyBufferHeader,
};
use chunk::MAX_RENDERED_LINE_LEN;
pub(crate) use chunk::{
    BlockChunks as DisplayChunks, DisplayRowEvent, HighlightStyles, RenderedWhitespace,
    chunk_to_run,
};
#[cfg(test)]
pub(crate) use chunk::{ChunkSource, ChunkText, WrapChunks};
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
    ChunkRenderer, ChunkRendererId, FoldBias, FoldPlaceholder, ProjectedLineIndex,
};
use fold_map::{FoldMap, FoldPointCursor, FoldSnapshot};
use gpui::{
    App, AppContext as _, Bounds, Context, Entity, HighlightStyle, Pixels, ShapedLine, TextRun,
    WindowTextSystem,
};
use tab_map::{TabMap, TabPointCursor};
pub(crate) use tab_map::{byte_for_display_column, display_column_for_byte};
use wrap_map::{WrapEdit, WrapMap, WrapPointCursor, WrapSnapshot};
use zcv_language::HighlightSpan;
use zcv_multi_buffer::{
    DiffDisplaySnapshot, ExcerptSnapshot, MultiBuffer, MultiBufferSnapshot, MultiBufferSource,
    MultiBufferSubscription,
};
use zcv_text::{
    Affinity, BufferId, Line, LineRange, LogicalColumn, MovementDirection, MovementUnit, Position,
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

/// 软换行层内的有序点对范围；只表达本层坐标，不借用 fold 的投影范围类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WrapRange {
    start: WrapPoint,
    end: WrapPoint,
}

impl WrapRange {
    pub(crate) const fn new(start: WrapPoint, end: WrapPoint) -> Self {
        Self { start, end }
    }

    pub(crate) const fn start(self) -> WrapPoint {
        self.start
    }

    pub(crate) const fn end(self) -> WrapPoint {
        self.end
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

/// 一个折叠候选：组合文档中的稳定锚点范围。
#[derive(Clone, Debug)]
pub(crate) struct Crease {
    range: Range<MultiBufferAnchor>,
}

impl Crease {
    pub(crate) fn simple(range: Range<MultiBufferAnchor>) -> Self {
        Self { range }
    }

    pub(crate) fn range(&self) -> &Range<MultiBufferAnchor> {
        &self.range
    }
}

/// 只缓存最近一次被 gutter 悬停请求的逻辑行范围。
///
/// 视口移动时保留重叠行的候选，只查询新进入范围的行，避免滚动遍历 Tree-sitter 全可见行；
/// 缓存随显示快照替换，并在每次查询后裁剪到当前范围，空间占用不会随滚动距离增长。
#[derive(Debug, Default)]
struct SyntaxCreaseIndex {
    covered: Option<Range<Line>>,
    by_line: BTreeMap<Line, Crease>,
}

impl SyntaxCreaseIndex {
    fn cover(
        &mut self,
        range: Range<Line>,
        mut creases_in: impl FnMut(Range<Line>) -> BTreeMap<Line, Crease>,
    ) {
        let Some(covered) = self.covered.clone() else {
            self.by_line.extend(creases_in(range.clone()));
            self.covered = Some(range);
            return;
        };

        if range.end < covered.start || covered.end < range.start {
            self.by_line.clear();
            self.by_line.extend(creases_in(range.clone()));
            self.covered = Some(range);
            return;
        }

        if range.start < covered.start {
            let missing_end = covered.start.min(range.end);
            if range.start < missing_end {
                self.by_line.extend(creases_in(range.start..missing_end));
            }
        }
        if covered.end < range.end {
            let missing_start = covered.end.max(range.start);
            if missing_start < range.end {
                self.by_line.extend(creases_in(missing_start..range.end));
            }
        }

        self.by_line.retain(|line, _| range.contains(line));
        self.covered = Some(range);
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
    /// 与本显示版本绑定的显示装饰投影；随快照整体替换、可丢弃。
    decorations: Arc<DisplayDecorations>,
    /// diff 显示输入归组合文档所有；这里只持有当前显示版本的不可变引用。
    diff_display: Option<Arc<DiffDisplaySnapshot>>,
    /// 仅在 gutter 悬停时填充，复用可见区间重叠部分的语法折叠候选。
    syntax_crease_cache: Arc<Mutex<SyntaxCreaseIndex>>,
    /// 显示版本；每次替换当前显示快照都会前进，后台派生结果据此判断是否过期。
    version: u64,
}

impl DisplaySnapshot {
    /// diff hunk 装饰的视口投影。
    pub(crate) fn diff_decorations(&self) -> Arc<DiffDecorationSnapshot> {
        self.decorations.diff(self)
    }

    /// 只投影与当前显示视口相交的 diff hunk。
    pub(crate) fn diff_decorations_for_viewport(
        &self,
        viewport: Range<usize>,
    ) -> Arc<DiffDecorationSnapshot> {
        self.decorations.diff_for_viewport(self, viewport)
    }

    /// 搜索命中装饰；无搜索时为空。
    pub(crate) fn search_decorations(&self) -> Option<Arc<SearchDecorationSnapshot>> {
        self.decorations.search()
    }

    /// 滚动条慢标记的轨道几何；由 Editor 在后台按显示版本计算并缓存。
    ///
    /// 标记来源由当前显示装饰决定，不按文档形态分流：
    /// 有 diff 装饰即投影 git 标记，有搜索装饰即投影搜索标记。
    /// 组合文档与单文档共用同一条标记链，差异只体现在各自实际持有的装饰上。
    pub(crate) fn scrollbar_marker_groups(
        &self,
        track_bounds: Bounds<Pixels>,
        scroll_per_pixel: f32,
        line_height: Pixels,
    ) -> [Option<Arc<[ScrollbarMarker]>>; 2] {
        let diff_markers = Some(Arc::from(
            marker_geometry(
                self.diff_decorations().scrollbar_marker_ranges(),
                track_bounds,
                scroll_per_pixel,
                line_height,
            )
            .into_boxed_slice(),
        ));
        let search_markers = self.search_decorations().map(|search| {
            Arc::from(
                marker_geometry(
                    search.scrollbar_marker_ranges(self),
                    track_bounds,
                    scroll_per_pixel,
                    line_height,
                )
                .into_boxed_slice(),
            )
        });
        [diff_markers, search_markers]
    }

    /// 返回指定逻辑行的折叠候选。
    ///
    /// 折叠候选只来自语法折叠范围，且只在该行成为交互目标时查询。
    pub(crate) fn crease_at_line(&self, line: Line) -> Option<Crease> {
        self.syntax_crease_at_line(line)
    }

    /// 只查询光标等交互目标行，不为整个可见视口生成语法折叠范围。
    pub(super) fn foldable_lines_at_lines(
        &self,
        lines: impl IntoIterator<Item = Line>,
    ) -> BTreeSet<Line> {
        lines
            .into_iter()
            .filter(|line| self.syntax_crease_at_line(*line).is_some())
            .collect()
    }

    /// gutter 悬停时查询视口内折叠行，并复用与上次视口相交的候选。
    pub(super) fn foldable_lines_in_range(&self, range: Range<Line>) -> BTreeSet<Line> {
        if range.is_empty() {
            return BTreeSet::new();
        }
        let mut index = self
            .syntax_crease_cache
            .lock()
            .expect("语法折叠候选缓存锁不得中毒");
        index.cover(range.clone(), |range| self.syntax_creases_in_range(range));
        index.by_line.range(range).map(|(line, _)| *line).collect()
    }

    fn syntax_crease_at_line(&self, line: Line) -> Option<Crease> {
        let buffer = self.buffer_snapshot();
        let cursor = buffer.line_cursor(line)?;
        let excerpt = cursor.excerpt_snapshot()?;
        let source = cursor.source()?;
        let source_text = source.text();
        let source_line = source_text.byte_to_line(source.source_offset()).ok()?;
        let source_line_start = source_text.line_start_byte(source_line).ok()?.get();
        let source_line_end = if source_line.get() + 1 < source_text.line_count() {
            source_text
                .line_start_byte(Line::new(source_line.get() + 1))
                .ok()?
                .get()
        } else {
            source_text.len_bytes().get()
        };
        let source_range = excerpt.source_range();
        let source_start = source_line_start.max(source_range.start().get());
        let source_end = source_line_end.min(source_range.end().get());
        if source_start >= source_end {
            return None;
        }

        source
            .syntax()
            .fold_ranges(source_start..source_end, source_text)
            .into_iter()
            .filter_map(|fold| {
                let start = fold.range.start.resolve_in(source_text).ok()?;
                let end = fold.range.end.resolve_in(source_text).ok()?;
                let range = source.project_range(start..end)?;
                let projected_line = buffer.byte_to_line(range.start).ok()?;
                (projected_line == line).then(|| {
                    let anchors = buffer.anchor_at(range.start, Affinity::Before)
                        ..buffer.anchor_at(range.end, Affinity::After);
                    (range.end, Crease::simple(anchors))
                })
            })
            .min_by_key(|(end, _)| *end)
            .map(|(_, crease)| crease)
    }

    fn syntax_creases_in_range(&self, range: Range<Line>) -> BTreeMap<Line, Crease> {
        let buffer = self.buffer_snapshot();
        let Some(mut cursor) = buffer.line_cursor(range.start) else {
            return BTreeMap::new();
        };
        let mut candidates: BTreeMap<Line, (usize, Crease)> = BTreeMap::new();
        let mut source_range = None;

        for row in range.start.get()..range.end.get() {
            let output_line = Line::new(row);
            if row > range.start.get() && !cursor.seek(output_line) {
                break;
            }
            let Some(excerpt) = cursor.excerpt_snapshot() else {
                Self::flush_syntax_fold_range(buffer, source_range.take(), &range, &mut candidates);
                continue;
            };
            let Some(source) = cursor.source() else {
                Self::flush_syntax_fold_range(buffer, source_range.take(), &range, &mut candidates);
                continue;
            };
            let Ok(source_line) = source.text().byte_to_line(source.source_offset()) else {
                Self::flush_syntax_fold_range(buffer, source_range.take(), &range, &mut candidates);
                continue;
            };
            let source_range_end = source_line.get() + 1;

            match source_range.as_mut() {
                Some((current_excerpt, output_lines, source_lines, _))
                    if *current_excerpt == excerpt && source_lines.end == source_line.get() =>
                {
                    output_lines.end = Line::new(row + 1);
                    source_lines.end = source_range_end;
                }
                _ => {
                    Self::flush_syntax_fold_range(
                        buffer,
                        source_range.take(),
                        &range,
                        &mut candidates,
                    );
                    source_range = Some((
                        excerpt,
                        output_line..Line::new(row + 1),
                        source_line.get()..source_range_end,
                        source,
                    ));
                }
            }
        }

        Self::flush_syntax_fold_range(buffer, source_range, &range, &mut candidates);
        candidates
            .into_iter()
            .map(|(line, (_, crease))| (line, crease))
            .collect()
    }

    fn flush_syntax_fold_range(
        buffer: &MultiBufferSnapshot,
        source_range: Option<(
            ExcerptSnapshot,
            Range<Line>,
            Range<usize>,
            MultiBufferSource<'_>,
        )>,
        visible_lines: &Range<Line>,
        candidates: &mut BTreeMap<Line, (usize, Crease)>,
    ) {
        let Some((excerpt, output_lines, source_lines, source)) = source_range else {
            return;
        };
        let source_text = source.text();
        let Ok(source_line_start) = source_text.line_start_byte(Line::new(source_lines.start))
        else {
            return;
        };
        let source_start = source_line_start
            .get()
            .max(excerpt.source_range().start().get());
        let source_end = if source_lines.end < source_text.line_count() {
            let Ok(source_line_end) = source_text.line_start_byte(Line::new(source_lines.end))
            else {
                return;
            };
            source_line_end
                .get()
                .min(excerpt.source_range().end().get())
        } else {
            excerpt.source_range().end().get()
        };
        if source_start >= source_end {
            return;
        }

        for fold in source
            .syntax()
            .fold_ranges(source_start..source_end, source_text)
        {
            let Some(range) = fold
                .range
                .start
                .resolve_in(source_text)
                .ok()
                .zip(fold.range.end.resolve_in(source_text).ok())
                .and_then(|(start, end)| source.project_range(start..end))
            else {
                continue;
            };
            let Ok(line) = buffer.byte_to_line(range.start) else {
                continue;
            };
            if !output_lines.contains(&line) || !visible_lines.contains(&line) {
                continue;
            }
            let end = range.end;
            let crease = Crease::simple(
                buffer.anchor_at(range.start, Affinity::Before)
                    ..buffer.anchor_at(range.end, Affinity::After),
            );
            candidates
                .entry(line)
                .and_modify(|(current_end, current)| {
                    if end.get() < *current_end {
                        *current_end = end.get();
                        *current = crease.clone();
                    }
                })
                .or_insert((end.get(), crease));
        }
    }

    pub(super) fn tab_width(&self) -> NonZeroUsize {
        self.wrap_snapshot().tab_snapshot().tab_width()
    }

    pub(super) fn has_expanded_buffers(&self) -> bool {
        self.block_snapshot.has_expanded_buffers()
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

    /// 指定逻辑行是否为折叠入口行（命令路径的点查询，不遍历全部折叠）。
    pub(super) fn is_fold_anchor_line(&self, line: Line) -> bool {
        self.fold_snapshot().is_fold_anchor_line(line)
    }

    pub(super) fn fold_anchor_lines_in_range(&self, line_range: Range<Line>) -> Vec<Line> {
        self.fold_snapshot().fold_anchor_lines_in_range(line_range)
    }

    /// 测试辅助：整份文档范围内的折叠入口行；生产路径只用按行/范围的点查询。
    #[cfg(test)]
    pub(super) fn fold_anchor_lines(&self) -> Vec<Line> {
        self.fold_snapshot()
            .fold_anchor_lines_in_range(Line::ZERO..Line::new(self.buffer_snapshot().line_count()))
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

    /// 将当前显示视口映射为覆盖其文本行的组合逻辑行范围。
    pub(super) fn logical_lines_for_display_rows(&self, rows: Range<usize>) -> Range<usize> {
        let buffer = self.buffer_snapshot();
        if rows.is_empty() {
            return 0..0;
        }
        let logical_line_at = |row| {
            let point = DisplayPoint::new(DisplayRow::new(row), DisplayColumn::ZERO);
            let offset = self
                .display_point_to_offset_with_bias(point, FoldBias::Left)
                .ok()?;
            buffer.byte_to_line(offset).ok().map(Line::get)
        };
        let start = logical_line_at(rows.start).unwrap_or(0);
        let end = logical_line_at(rows.end - 1)
            .map_or(buffer.line_count(), |line| line.saturating_add(1));
        start.min(end)..end.min(buffer.line_count())
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

    /// 垂直移动与可见行共用窗口化 chunk 和字体测量，只塑形目标附近的文本。
    fn layout_row_for_movement(
        &self,
        row: DisplayRow,
        window_columns: (usize, usize),
        text_system: &WindowTextSystem,
        font_size: Pixels,
        base: &TextRun,
        cx: &App,
    ) -> Option<(usize, Pixels, String, ShapedLine)> {
        if row.get() >= self.line_count() {
            return None;
        }
        let range = row..DisplayRow::new(row.get() + 1);
        let source_ranges = self
            .chunks(range.clone(), HighlightStyles::default(), None)
            .source_line_ranges();
        let spans = self.highlighted_spans_for_source_ranges(source_ranges);
        let styles = self.highlight_styles(cx);
        let mut text = String::new();
        let mut runs = Vec::new();
        let mut start_column = 0;
        let mut prefix_width = Pixels::ZERO;
        self.chunks(
            range,
            HighlightStyles {
                spans: &spans,
                styles: &styles,
                ..Default::default()
            },
            Some(window_columns),
        )
        .for_each_row(|event| {
            if let DisplayRowEvent::Text { row, chunks } = event {
                start_column = row.window_start_column;
                if !row.window_prefix.is_empty() {
                    prefix_width = text_system
                        .shape_line(
                            row.window_prefix.to_string().into(),
                            font_size,
                            &[TextRun {
                                len: row.window_prefix.len(),
                                ..base.clone()
                            }],
                            None,
                        )
                        .width;
                }
                if row.indent > 0 {
                    text.push_str(&" ".repeat(row.indent));
                    runs.push(TextRun {
                        len: row.indent,
                        ..base.clone()
                    });
                }
                for chunk in chunks {
                    text.push_str(chunk.text);
                    runs.push(chunk_to_run(&chunk, base.clone()));
                }
            }
        });
        let layout = text_system.shape_line(text.clone().into(), font_size, &runs, None);
        Some((start_column, prefix_width, text, layout))
    }

    pub(crate) fn x_for_display_point(
        &self,
        point: DisplayPoint,
        text_system: &WindowTextSystem,
        font_size: Pixels,
        base: &TextRun,
        cx: &App,
    ) -> Option<Pixels> {
        let column = point.column().get();
        let (start_column, prefix_width, text, layout) = self.layout_row_for_movement(
            point.row(),
            (column.saturating_sub(32), column.saturating_add(32)),
            text_system,
            font_size,
            base,
            cx,
        )?;
        let byte = byte_for_display_column(&text, start_column, column, self.tab_width().get());
        Some(prefix_width + layout.x_for_index(byte))
    }

    pub(crate) fn display_column_for_x(
        &self,
        row: DisplayRow,
        x: Pixels,
        text_system: &WindowTextSystem,
        font_size: Pixels,
        base: &TextRun,
        cx: &App,
    ) -> Option<DisplayColumn> {
        let font_id = text_system.resolve_font(&base.font);
        let em_advance = text_system.em_advance(font_id, font_size).ok()?;
        let mut start = ((x / em_advance).floor() as usize).saturating_sub(32);
        loop {
            let end = start.saturating_add(256);
            let (start_column, prefix_width, text, layout) =
                self.layout_row_for_movement(row, (start, end), text_system, font_size, base, cx)?;
            if x < prefix_width && start > 0 {
                start /= 2;
                continue;
            }
            let end_column =
                display_column_for_byte(&text, start_column, text.len(), self.tab_width().get());
            if x > prefix_width + layout.width
                && !text.is_empty()
                && (end_column >= end || text.len() >= MAX_RENDERED_LINE_LEN)
            {
                start = end_column.saturating_sub(16).max(start.saturating_add(1));
                continue;
            }
            let byte = layout.closest_index_for_x(x - prefix_width);
            return Some(DisplayColumn::new(display_column_for_byte(
                &text,
                start_column,
                byte,
                self.tab_width().get(),
            )));
        }
    }

    pub(super) fn project_text_range(
        &self,
        range: MultiBufferRange,
    ) -> DisplayMapResult<Vec<DisplayRange>> {
        self.block_snapshot.project_text_range(range)
    }

    pub(crate) fn display_point_converter(&self) -> DisplayPointConverter<'_> {
        DisplayPointConverter::new(self)
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

/// 按非递减组合偏移把范围逐层投影到显示坐标；每层游标跨范围复用 SumTree 定位。
pub(crate) struct DisplayPointConverter<'a> {
    buffer: MultiBufferPositionCursor<'a>,
    fold: FoldPointCursor<'a>,
    tab: TabPointCursor<'a>,
    wrap: WrapPointCursor<'a>,
    block: BlockPointCursor<'a>,
    previous_end: Option<MultiBufferOffset>,
}

impl<'a> DisplayPointConverter<'a> {
    fn new(snapshot: &'a DisplaySnapshot) -> Self {
        let block_snapshot = snapshot.block_snapshot.as_ref();
        let wrap_snapshot = block_snapshot.wrap_snapshot();
        let tab_snapshot = wrap_snapshot.tab_snapshot();
        let fold_snapshot = tab_snapshot.fold_snapshot();
        Self {
            buffer: MultiBufferPositionCursor::new(fold_snapshot.buffer_snapshot()),
            fold: fold_snapshot.point_cursor(),
            tab: tab_snapshot.point_cursor(),
            wrap: wrap_snapshot.point_cursor(),
            block: block_snapshot.point_cursor(),
            previous_end: None,
        }
    }

    pub fn reset(&mut self) {
        self.buffer.reset();
        self.fold.reset();
        self.tab.reset();
        self.wrap.reset();
        self.block.reset();
        self.previous_end = None;
    }

    pub fn map(&mut self, range: MultiBufferRange) -> DisplayMapResult<Option<DisplayRange>> {
        if self
            .previous_end
            .is_some_and(|previous_end| range.start() < previous_end)
        {
            self.reset();
        }
        self.previous_end = Some(range.end());
        if range.start() == range.end() {
            return Ok(None);
        }

        let start_position = self.buffer.byte_to_position(range.start())?;
        let end_position = self.buffer.byte_to_position(range.end())?;
        let start_fold = self.fold.map(start_position.into(), FoldBias::Left)?;
        let end_fold = self.fold.map(end_position.into(), FoldBias::Right)?;
        let start_tab = self.tab.map(start_fold);
        let end_tab = self.tab.map(end_fold);
        let start_wrap = self.wrap.map(start_tab)?;
        let end_wrap = self.wrap.map(end_tab)?;
        let start = self.block.map(start_wrap);
        let end = self.block.map(end_wrap);
        let ordered =
            start.row() < end.row() || (start.row() == end.row() && start.column() < end.column());
        if !ordered {
            return Ok(None);
        }
        Ok(Some(DisplayRange::new(start, end)))
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
    folded_buffers: Arc<HashSet<BufferId>>,
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
            folded_buffers: Arc::new(HashSet::new()),
            snapshot: None,
            multi_buffer: None,
            buffer_subscription: None,
            editor_hunks: Arc::from([]),
            search: None,

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
    /// 语法折叠候选不参与显示拓扑同步，只在交互需要时从当前快照查询。
    pub(crate) fn snapshot(&mut self, cx: &mut Context<Self>) -> DisplaySnapshot {
        let Some(multi_buffer) = self.multi_buffer.clone() else {
            let tab_snapshot = self.tab_map.snapshot().clone();
            let (wrap_snapshot, wrap_edits) = self
                .wrap_map
                .update(cx, |map, cx| map.sync(tab_snapshot, &[], cx));
            self.commit_snapshot(&wrap_snapshot, &wrap_edits, cx);
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

    /// 只替换宿主 hunk 影响的 diff 域装饰；搜索域与显示拓扑保持不变。
    fn refresh_editor_hunk_decorations(&mut self, cx: &mut Context<Self>) {
        let Some(mut snapshot) = self.snapshot.take() else {
            return;
        };
        snapshot.decorations = Arc::new(
            snapshot
                .decorations
                .with_diff_inputs(Arc::clone(&self.editor_hunks)),
        );
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
        cached_diff: Option<Arc<DiffDecorationSnapshot>>,
        _cx: &App,
    ) -> DisplayDecorations {
        DisplayDecorations::new(
            self.search.as_ref(),
            Arc::clone(&self.editor_hunks),
            cached_diff,
        )
    }

    fn commit_snapshot(&mut self, wrap_snapshot: &WrapSnapshot, wrap_edits: &[WrapEdit], cx: &App) {
        if wrap_edits.is_empty()
            && self.snapshot.as_ref().is_some_and(|previous| {
                let old = previous.wrap_snapshot();
                let old_buffer = old.buffer_snapshot();
                let new_buffer = wrap_snapshot.buffer_snapshot();
                old.version() == wrap_snapshot.version()
                    && old.tab_snapshot().version() == wrap_snapshot.tab_snapshot().version()
                    && old_buffer.version() == new_buffer.version()
                    && old_buffer.metadata_version() == new_buffer.metadata_version()
                    && old_buffer.topology_version() == new_buffer.topology_version()
                    && same_diff_display(old_buffer.diff_display(), new_buffer.diff_display())
                    && previous
                        .block_snapshot
                        .folded_buffers_match(&self.folded_buffers)
            })
        {
            return;
        }
        let block_snapshot = Arc::new(self.current_block_snapshot(wrap_snapshot, wrap_edits));
        let version = self.next_display_version();
        let mut snapshot = DisplaySnapshot {
            block_snapshot,
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
            .then(|| previous.decorations.cached_diff())
            .flatten()
        });
        snapshot.decorations = Arc::new(self.build_decorations(cached_diff, cx));
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

    pub(crate) fn set_buffers_folded(
        &mut self,
        ids: impl IntoIterator<Item = BufferId>,
        folded: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut changed = false;
        for id in ids {
            if self.folded_buffers.contains(&id) != folded {
                changed = true;
                let buffers = Arc::make_mut(&mut self.folded_buffers);
                if folded {
                    buffers.insert(id);
                } else {
                    buffers.remove(&id);
                }
            }
        }
        if changed {
            let tab_snapshot = self.tab_map.snapshot().clone();
            let (wrap_snapshot, wrap_edits) = self
                .wrap_map
                .update(cx, |map, cx| map.sync(tab_snapshot, &[], cx));
            self.commit_snapshot(&wrap_snapshot, &wrap_edits, cx);
        }
        changed
    }

    /// 设置软换行宽度与字体；变化时启动预算内重排，经同步入口消费净编辑。
    pub(crate) fn set_wrap_width(
        &mut self,
        wrap_width: Option<gpui::Pixels>,
        font: gpui::Font,
        font_size: gpui::Pixels,
        text_system: &std::sync::Arc<gpui::TextSystem>,
        cx: &mut Context<Self>,
    ) -> bool {
        let changed = self.wrap_map.update(cx, |map, cx| {
            map.set_wrap_width(wrap_width, font, font_size, text_system.clone(), cx)
        });
        if changed {
            let tab_snapshot = self.tab_map.snapshot().clone();
            let (wrap_snapshot, wrap_edits) = self
                .wrap_map
                .update(cx, |map, cx| map.sync(tab_snapshot, &[], cx));
            self.commit_snapshot(&wrap_snapshot, &wrap_edits, cx);
        }
        changed
    }

    /// 回写渲染层实测的元素宽度；宽度变化时经零宽 FoldEdit 逐层推进显示链。
    ///
    /// 渲染层是实测宽度的唯一来源，折叠层是回写后的唯一权威；
    /// 返回是否发生变化，渲染层据此决定是否用新快照重新布局本帧。
    pub(crate) fn update_fold_widths(
        &mut self,
        widths: impl IntoIterator<Item = (ChunkRendererId, Pixels)>,
        cx: &mut Context<Self>,
    ) -> bool {
        let (fold_snapshot, fold_edits) = self.fold_map.write().update_fold_widths(widths);
        if fold_edits.is_empty() {
            return false;
        }
        let tab_width = self.tab_map.snapshot().tab_width();
        let (tab_snapshot, tab_edits) = self.tab_map.sync(fold_snapshot, &fold_edits, tab_width);
        let (wrap_snapshot, wrap_edits) = self
            .wrap_map
            .update(cx, |map, cx| map.sync(tab_snapshot, &tab_edits, cx));
        self.commit_snapshot(&wrap_snapshot, &wrap_edits, cx);
        true
    }

    /// 未开启软换行时读取 Wrap 层 summary 中的最长行。
    ///
    /// 最长行是显示投影的派生维度，由 `isomorphic_tree` 构建 summary 时测量一次；
    /// 这里只做 O(1) 读取与 wrap 行 → 显示行换算，不在每帧扫描全部行。
    pub(crate) fn longest_unwrapped_row(&self) -> DisplayRow {
        let snapshot = self
            .snapshot
            .as_ref()
            .expect("DisplayMap 初始化后必须存在显示快照");
        let row = snapshot.block_snapshot.wrap_snapshot().longest_row();
        snapshot
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
        // BlockMap 只能消费同一条下层快照链中的事实。
        // 换行层可能仍处于上一帧的急切插值快照；
        // 此时从当前 FoldMap 另取 excerpts 会把两个版本混进同一次块投影同步，导致块锚点和变换输入空间不再对应。
        // 消费换行编辑流：块布局未变时复用，几何变化时按显式分支重建。
        match &self.snapshot {
            Some(previous) => previous.block_snapshot.sync(
                wrap_snapshot.clone(),
                &self.folded_buffers,
                wrap_edits,
            ),
            None => BlockSnapshot::new(wrap_snapshot.clone(), &self.folded_buffers),
        }
    }
}

#[cfg(test)]
#[path = "display_map/test/support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "test/display_map_tests.rs"]
mod tests;
