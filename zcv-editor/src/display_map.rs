//! 决定 Buffer 文本如何映射到 Editor 的显示坐标。
//!
//! DisplayMap 由一组自底向上的变换层组成：
//! - InlayMap：在 Buffer 文本中投影行内提示；
//! - FoldMap：维护折叠范围和折叠后的文本拓扑；
//! - TabMap：在 FoldSnapshot 之上处理硬 Tab 的显示列；
//! - WrapMap：在 TabSnapshot 之上按像素宽度软换行。
//! - BlockMap：在换行结果上插入文件标题、片段分隔线等非文本虚拟块。
//!
//! 每一层都持有自己的 Map 和不可变 Snapshot；
//! 上一层 Snapshot 固化下一层 Snapshot，从而让一次渲染只能看到一条内部一致的显示状态。

mod block_map;
mod chunk;
mod display_width;
mod error;
mod fold_map;
mod inlay_map;
mod line_stream;
mod tab_map;
mod wrap_map;

use std::borrow::Cow;
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
pub(crate) use display_width::DisplayColumn;
use error::DisplayMapResult;
#[cfg(test)]
pub(crate) use fold_map::ProjectedPoint;
use fold_map::{ApplyOutcome, FoldMap, FoldSnapshot, LogicalProjection};
pub(crate) use fold_map::{FoldBias, FoldRowSegment, ProjectedLineIndex, ProjectedRange};
use gpui::{App, Context, Entity, EventEmitter, HighlightStyle};
pub(crate) use inlay_map::Inlay;
use inlay_map::InlayMap;
use line_stream::LineStream;
use tab_map::TabMap;
pub(crate) use tab_map::{byte_for_display_column, display_column_for_byte};
pub(crate) use wrap_map::WrapRowKind;
use wrap_map::{WrapEdit, WrapMap, WrapSnapshot};
use zcv_language::HighlightSpan;
use zcv_multi_buffer::{MultiBuffer, MultiBufferSnapshot, MultiBufferSubscription};
use zcv_text::{
    ByteOffset, Line, LineRange, LogicalColumn, MovementDirection, MovementUnit, Position,
    TextChangeBatch, TextRange, TextResult,
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

/// 一帧渲染使用的只读显示快照。
///
/// FoldSnapshot、TabSnapshot 与 WrapSnapshot 都是低成本克隆；渲染持有此值时
/// 不会阻塞 Editor 接收后续 Buffer 更新。主题样式不属于显示快照，渲染需要时按当前主题派生。
#[derive(Debug, Clone)]
pub(super) struct DisplaySnapshot {
    /// 显示拓扑版本；任何会改变逻辑行到显示行映射的操作都会推进它。
    revision: u64,
    block_snapshot: Arc<BlockSnapshot>,
    multi_buffer_snapshot: MultiBufferSnapshot,
    /// 语法快照提供的 capture 名字表；主题样式由调用方按需解析。
    capture_names: std::sync::Arc<[std::sync::Arc<str>]>,
}

impl DisplaySnapshot {
    pub(super) const fn revision(&self) -> u64 {
        self.revision
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
            spans.extend(self.multi_buffer_snapshot.highlights(range.clone()));
        }
        Arc::from(spans)
    }

    /// 按当前主题生成 capture 索引 → 样式的预展开表。
    pub(super) fn highlight_styles(&self) -> Vec<HighlightStyle> {
        syntax::style_table(&self.capture_names)
    }

    #[cfg(test)]
    pub(super) fn version(&self) -> u64 {
        self.wrap_snapshot().version()
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
        offset: ByteOffset,
    ) -> Option<(ByteOffset, ByteOffset)> {
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

    pub(crate) fn row_text(&self, projected_line: usize) -> Option<Cow<'_, str>> {
        let row = ProjectedLineIndex::new(projected_line);
        let fold = self.fold_snapshot();
        if fold.is_fold_row(row) {
            fold.row_text(row)
        } else {
            self.wrap_snapshot()
                .tab_snapshot()
                .line_text(Line::new(projected_line))
        }
    }

    /// 从显示快照的起点连续消费 Block/Fold/Wrap/Inlay 产生的 chunk。
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
        range: TextRange,
    ) -> DisplayMapResult<Vec<ProjectedRange>> {
        self.block_snapshot.project_text_range(range)
    }

    pub(super) fn offset_to_display_point(
        &self,
        offset: ByteOffset,
    ) -> DisplayMapResult<DisplayPoint> {
        self.block_snapshot.offset_to_display_point(offset)
    }

    pub(super) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<ByteOffset> {
        self.block_snapshot.display_point_to_offset(point)
    }

    pub(super) fn display_point_to_offset_with_bias(
        &self,
        point: DisplayPoint,
        bias: FoldBias,
    ) -> DisplayMapResult<ByteOffset> {
        self.block_snapshot
            .display_point_to_offset_with_bias(point, bias)
    }

    pub(super) fn sticky_buffer_header(
        &self,
        top_row: DisplayRow,
    ) -> Option<block_map::StickyBufferHeader> {
        self.block_snapshot.sticky_buffer_header(top_row)
    }

    /// 语法查询读取元数据当前的组合快照。
    ///
    /// `sync_metadata` 只替换语法附属数据并复用既有 Block 快照，因此 `buffer_snapshot()`（Block 链）
    /// 可能滞后于文本不变的语法重解析；这里必须读取字段本身。
    pub(super) fn ancestor_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        self.multi_buffer_snapshot.expand_selection_range(range)
    }

    /// 查询指定组合文档范围的语法高亮，并解析为当前编辑器主题样式。
    pub(super) fn highlights_for_range(
        &self,
        range: Range<usize>,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let highlight_styles = syntax::style_table(&self.capture_names);
        self.multi_buffer_snapshot
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
        offset: ByteOffset,
        direction: MovementDirection,
        unit: MovementUnit,
    ) -> TextResult<ByteOffset> {
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

    pub(super) fn beginning_of_row(&self, offset: ByteOffset) -> DisplayMapResult<ByteOffset> {
        self.wrap_snapshot().beginning_of_row(offset)
    }

    pub(super) fn end_of_row(&self, offset: ByteOffset) -> DisplayMapResult<ByteOffset> {
        self.wrap_snapshot().end_of_row(offset)
    }
}

/// DisplayMap 向订阅者广播的显示管线变化。
#[derive(Clone, Debug)]
pub(crate) struct DisplayMapEvent {
    pub(crate) changes: TextChangeBatch,
}

impl EventEmitter<DisplayMapEvent> for DisplayMap {}

#[derive(Debug, Clone)]
pub(crate) struct DisplayMap {
    /// 显示映射的派生状态版本。滚动不改变它，换行、折叠和文本同步才会推进它。
    revision: u64,
    inlay_map: InlayMap,
    fold_map: FoldMap,
    tab_map: TabMap,
    wrap_map: WrapMap,
    multi_buffer_snapshot: MultiBufferSnapshot,
    /// 行内提示配置（inlay 注入；变化时整链重建）。
    inlays: Vec<Inlay>,
    /// 语法快照提供的 capture 名字表；主题样式不在 DisplayMap 中持有。
    capture_names: std::sync::Arc<[std::sync::Arc<str>]>,
    /// 由 BufferHeader 控制的整文件折叠；BlockMap 在 WrapMap 之上隐藏对应文本行。
    folded_buffers: HashSet<PathBuf>,
    /// 当前显示管线的持久派生快照；滚动和普通重绘只克隆快照，不重建 BlockSnapshot。
    snapshot: Option<DisplaySnapshot>,
    /// 组合文本源：DisplayMap 是组合文本变更与同步的唯一持有者。
    multi_buffer: Option<Entity<MultiBuffer>>,
    buffer_subscription: Option<MultiBufferSubscription>,
}

impl DisplayMap {
    pub(crate) fn new(snapshot: impl Into<MultiBufferSnapshot>) -> Self {
        let snapshot = snapshot.into();
        let stream = LineStream::new(snapshot.clone());
        let (inlay_map, inlay_snapshot) = InlayMap::new(stream);
        let (fold_map, fold_snapshot) = FoldMap::new(inlay_snapshot);
        let (tab_map, tab_snapshot) = TabMap::new(fold_snapshot);
        let (wrap_map, wrap_snapshot) = WrapMap::new(tab_snapshot);
        let _ = wrap_snapshot;
        let mut this = Self {
            revision: 0,
            inlay_map,
            fold_map,
            tab_map,
            wrap_map,
            multi_buffer_snapshot: snapshot.clone(),
            inlays: Vec::new(),
            capture_names: std::sync::Arc::from([]),
            folded_buffers: HashSet::new(),
            snapshot: None,
            multi_buffer: None,
            buffer_subscription: None,
        };
        this.set_capture_names(snapshot.capture_names());
        this.refresh_snapshot(&[]);
        this
    }

    /// 绑定组合文档并订阅其变化；此后 DisplayMap 自行消费文本变更并推进显示管线。
    pub(crate) fn set_multi_buffer(
        &mut self,
        multi_buffer: Entity<MultiBuffer>,
        subscription: MultiBufferSubscription,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe(&multi_buffer, |map, _, _, cx| {
            let changes = map.sync_from_multi_buffer(cx);
            cx.emit(DisplayMapEvent { changes });
            cx.notify();
        })
        .detach();
        self.multi_buffer = Some(multi_buffer);
        self.buffer_subscription = Some(subscription);
    }

    /// 消费自上次同步以来的组合文本变化，并推进显示管线。
    ///
    /// 返回本次消费的文本变化，供 Editor 推进它自己的投影派生状态（搜索锚点等）。
    pub(crate) fn sync_from_multi_buffer(&mut self, cx: &App) -> TextChangeBatch {
        let snapshot = self
            .multi_buffer
            .as_ref()
            .expect("DisplayMap 必须绑定组合文档")
            .read(cx)
            .snapshot(cx);
        let changes = self
            .buffer_subscription
            .as_ref()
            .map_or_else(TextChangeBatch::default, |subscription| {
                subscription.consume()
            });
        if changes.is_empty() {
            if self.has_current_snapshot(&snapshot) {
            } else if self.has_text_snapshot(&snapshot) {
                self.sync_metadata(snapshot);
            } else {
                self.sync(snapshot, changes.clone());
            }
            return changes;
        }
        self.sync(snapshot, changes.clone());
        changes
    }

    fn set_capture_names(&mut self, capture_names: std::sync::Arc<[std::sync::Arc<str>]>) {
        if self.capture_names == capture_names {
            return;
        }
        self.capture_names = Arc::clone(&capture_names);
        // capture 表只影响样式解析，不改变显示拓扑：就地推进当前帧快照，
        // 保证 set_capture_names 之后 snapshot 不再暴露旧表。
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.capture_names = capture_names;
        }
    }

    /// 当前显示拓扑是否已经消费了指定的组合文本版本。
    ///
    /// 这只比较文本版本：语法结果和 capture 表可以在文本不变时更新，它们不应触发 Block/Fold/Wrap 重建，而是由 [`Self::sync_metadata`] 替换快照中的只读附属数据。
    pub(crate) fn has_text_snapshot(&self, snapshot: &MultiBufferSnapshot) -> bool {
        self.multi_buffer_snapshot.version() == snapshot.version()
    }

    /// 当前快照是否已完整反映指定组合快照。
    ///
    /// 普通渲染帧会取得一份新的 `MultiBufferSnapshot` 句柄；
    /// 只要它描述的是相同的文本、语法和 capture 表，就继续直接复用 `DisplaySnapshot`，不分配新的外壳。
    pub(crate) fn has_current_snapshot(&self, snapshot: &MultiBufferSnapshot) -> bool {
        self.has_text_snapshot(snapshot)
            && self.multi_buffer_snapshot.syntax_version() == snapshot.syntax_version()
            && self.capture_names.as_ref() == snapshot.capture_names().as_ref()
    }

    /// 替换不改变显示拓扑的快照附属数据。
    ///
    /// 语法重解析、主题 capture 表更新等只影响高亮查询。
    /// 它们共享既有 `BlockSnapshot`，因此滚动帧不会因为元数据同步而重新构造 Block/Fold/Wrap 投影或推进显示坐标版本。
    pub(crate) fn sync_metadata(&mut self, snapshot: MultiBufferSnapshot) {
        self.multi_buffer_snapshot = snapshot.clone();
        self.set_capture_names(snapshot.capture_names());
        let previous = self
            .snapshot
            .as_ref()
            .expect("DisplayMap 初始化后必须存在显示快照");
        self.snapshot = Some(DisplaySnapshot {
            revision: previous.revision,
            block_snapshot: Arc::clone(&previous.block_snapshot),
            multi_buffer_snapshot: snapshot,
            capture_names: Arc::clone(&self.capture_names),
        });
    }

    pub(super) fn snapshot(&self) -> DisplaySnapshot {
        self.snapshot
            .as_ref()
            .expect("DisplayMap 初始化后必须存在显示快照")
            .clone()
    }

    fn refresh_snapshot(&mut self, wrap_edits: &[WrapEdit]) {
        self.snapshot = Some(DisplaySnapshot {
            revision: self.revision,
            block_snapshot: Arc::new(self.current_block_snapshot(wrap_edits)),
            multi_buffer_snapshot: self.multi_buffer_snapshot.clone(),
            capture_names: std::sync::Arc::clone(&self.capture_names),
        });
    }

    pub(crate) fn is_buffer_folded(&self, path: &Path) -> bool {
        self.folded_buffers.contains(path)
    }

    pub(crate) fn set_buffer_folded(&mut self, path: PathBuf, folded: bool) {
        let changed = if folded {
            self.folded_buffers.insert(path)
        } else {
            self.folded_buffers.remove(&path)
        };
        if changed {
            self.revision = self.revision.wrapping_add(1);
            self.refresh_snapshot(&[]);
        }
    }

    /// 设置软换行宽度与字体；宽度/字体变化时内部重建，返回是否发生变化。
    pub(crate) fn set_wrap_width(
        &mut self,
        wrap_width: Option<gpui::Pixels>,
        font: gpui::Font,
        font_size: gpui::Pixels,
        text_system: &std::sync::Arc<gpui::TextSystem>,
    ) -> bool {
        let (changed, wrap_edits) =
            self.wrap_map
                .set_wrap_width(wrap_width, font, font_size, text_system.clone());
        if changed {
            self.revision = self.revision.wrapping_add(1);
            self.refresh_snapshot(&wrap_edits);
        }
        changed
    }

    pub(crate) fn measure_rows(
        &mut self,
        start_row: DisplayRow,
        line_count: usize,
    ) -> DisplayMapResult<()> {
        let end = start_row
            .get()
            .saturating_add(line_count)
            .min(self.snapshot().line_count());
        let block_snapshot = &self
            .snapshot
            .as_ref()
            .expect("DisplayMap 初始化后必须存在显示快照")
            .block_snapshot;
        let wrap_snapshot = self.wrap_map.snapshot();
        let tab_rows = (start_row.get()..end)
            .filter_map(|display_row| {
                let wrap_row =
                    block_snapshot.display_row_to_wrap_row(DisplayRow::new(display_row))?;
                Some(wrap_snapshot.tab_row_for_wrap_row(wrap_row))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for tab_row in tab_rows {
            self.tab_map.measure_line(tab_row)?;
        }
        Ok(())
    }

    pub(crate) fn longest_measured_row(&self) -> DisplayRow {
        let Some((line, _)) = self.tab_map.longest_measured() else {
            return DisplayRow::ZERO;
        };
        let Some(LogicalProjection::Visible(row)) =
            self.fold_map.snapshot().logical_to_projected(line).ok()
        else {
            return DisplayRow::ZERO;
        };
        self.snapshot
            .as_ref()
            .expect("DisplayMap 初始化后必须存在显示快照")
            .block_snapshot
            .projected_wrap_row_to_display_row(row.get())
    }

    /// 用订阅者独立积累的组合 Patch，把整条显示管线直接推进到当前 Snapshot。
    pub(crate) fn sync(
        &mut self,
        current_snapshot: impl Into<MultiBufferSnapshot>,
        batch: TextChangeBatch,
    ) -> ApplyOutcome {
        self.revision = self.revision.wrapping_add(1);
        let current_snapshot = current_snapshot.into();
        self.set_capture_names(current_snapshot.capture_names());
        self.multi_buffer_snapshot = current_snapshot.clone();
        let stream = LineStream::new(current_snapshot.clone());
        let inlay_snapshot = self.inlay_map.read(stream, self.inlays.clone());
        let (fold_snapshot, fold_edits, outcome) = self.fold_map.read(inlay_snapshot, &batch);
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &fold_edits);
        let wrap_edits = self.wrap_map.sync(tab_snapshot, &fold_edits);
        self.refresh_snapshot(&wrap_edits);
        outcome
    }

    /// 折叠字节范围（入口行行尾换行符 → 闭合括号前；闭合括号保留可见）。
    pub(crate) fn fold_range(&mut self, range: TextRange) -> DisplayMapResult<()> {
        let (fold_snapshot, fold_edits) = self.fold_map.write().fold(range)?;
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &fold_edits);
        let wrap_edits = self.wrap_map.sync(tab_snapshot, &fold_edits);
        self.revision = self.revision.wrapping_add(1);
        self.refresh_snapshot(&wrap_edits);
        Ok(())
    }

    /// 展开与行范围交叠的全部折叠（半开区间）。
    pub(crate) fn unfold_lines(&mut self, line_range: LineRange) -> DisplayMapResult<()> {
        let (fold_snapshot, fold_edits) = self.fold_map.write().unfold_lines(line_range)?;
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &fold_edits);
        let wrap_edits = self.wrap_map.sync(tab_snapshot, &fold_edits);
        self.revision = self.revision.wrapping_add(1);
        self.refresh_snapshot(&wrap_edits);
        Ok(())
    }

    fn current_block_snapshot(&self, wrap_edits: &[WrapEdit]) -> BlockSnapshot {
        let wrap_snapshot = self.wrap_map.snapshot().clone();
        let excerpts = self.multi_buffer_snapshot.excerpts_arc();
        // 消费换行编辑流：块起始不变时复用或平移块布局，只重排受编辑影响的块。
        if let Some(previous) = &self.snapshot
            && let Some(resynced) = previous.block_snapshot.resync(
                wrap_snapshot.clone(),
                excerpts.clone(),
                &self.folded_buffers,
                wrap_edits,
            )
        {
            return resynced;
        }
        BlockSnapshot::new(wrap_snapshot, excerpts, &self.folded_buffers)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use gpui::{TestAppContext, font, px};
    use zcv_text::{Buffer, BufferConfig, Edit, Line, TextRange, TransactionMetadata};
    use zcv_theme::ThemeChoice;

    use super::fold_map::ProjectedPoint;
    use super::*;

    fn rebuild_from_stream(map: &mut DisplayMap, stream: LineStream) -> Vec<WrapEdit> {
        let inlay_snapshot = map.inlay_map.read(stream, map.inlays.clone());
        let (fold_snapshot, fold_edits, _) = map
            .fold_map
            .read(inlay_snapshot, &TextChangeBatch::default());
        let tab_snapshot = map.tab_map.sync(fold_snapshot, &fold_edits);
        map.wrap_map.sync(tab_snapshot, &fold_edits)
    }

    fn set_inlays(map: &mut DisplayMap, inlays: Vec<Inlay>) {
        if map.inlays == inlays {
            return;
        }
        map.inlays = inlays;
        let stream = map.fold_map.snapshot().stream().clone();
        let wrap_edits = rebuild_from_stream(map, stream);
        map.refresh_snapshot(&wrap_edits);
    }

    fn apply_test_theme(cx: &mut TestAppContext, id: &'static str) {
        cx.update(|cx| ThemeChoice::Named(id).apply(cx, None));
    }

    #[gpui::test]
    fn display_snapshot_resolves_syntax_styles_from_current_theme(cx: &mut TestAppContext) {
        apply_test_theme(cx, "light");
        let buffer = Buffer::scratch("paragraph".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.set_capture_names(std::sync::Arc::from([std::sync::Arc::from("text")]));

        let light = map.snapshot().highlight_styles()[0].color;
        apply_test_theme(cx, "dark");
        let dark = map.snapshot().highlight_styles()[0].color;

        assert_ne!(light, dark, "同一 DisplayMap 应按当前主题重新派生语法颜色");
    }

    #[test]
    fn metadata_sync_reuses_the_display_topology_snapshot() {
        let buffer = Buffer::scratch("paragraph".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        let before = map.snapshot();
        let current = MultiBufferSnapshot::from(buffer.snapshot());

        assert!(
            map.has_current_snapshot(&current),
            "相同的组合快照必须直接复用 DisplaySnapshot"
        );

        map.sync_metadata(current);

        let after = map.snapshot();
        assert_eq!(after.revision(), before.revision());
        assert!(
            Arc::ptr_eq(&after.block_snapshot, &before.block_snapshot),
            "纯元数据同步不得重建 Block/Fold/Wrap 显示拓扑"
        );
    }

    #[test]
    fn projection_map_roundtrips_unicode_buffer_points_and_byte_offsets() {
        let buffer = Buffer::scratch("a你😀\nβ".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = DisplayMap::new(buffer.snapshot());
        let cases = [
            ByteOffset::new(0),
            ByteOffset::new(1),
            ByteOffset::new(4),
            ByteOffset::new(8),
            ByteOffset::new(9),
            ByteOffset::new(11),
        ];

        for offset in cases {
            let display_point = map
                .snapshot()
                .offset_to_display_point(offset)
                .expect("合法字节偏移应能映射");
            assert_eq!(
                map.snapshot()
                    .buffer_snapshot()
                    .byte_to_position(
                        map.snapshot()
                            .display_point_to_offset(display_point)
                            .expect("合法显示点应能还原"),
                    )
                    .expect("合法显示点应能还原"),
                map.snapshot()
                    .buffer_snapshot()
                    .byte_to_position(offset)
                    .expect("合法字节偏移应能转换为位置")
            );
            assert_eq!(
                map.snapshot()
                    .offset_to_display_point(offset)
                    .expect("合法字节偏移应能映射"),
                display_point
            );
            assert_eq!(
                map.snapshot()
                    .display_point_to_offset(display_point)
                    .expect("合法 DisplayPoint 应能转回 ByteOffset"),
                offset
            );
        }
    }

    #[test]
    fn projection_map_uses_display_columns_for_tabs() {
        let buffer = Buffer::scratch("\tx".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = DisplayMap::new(buffer.snapshot());

        let after_tab = map
            .snapshot()
            .offset_to_display_point(ByteOffset::new(1))
            .expect("tab 后的偏移应能映射");
        assert_eq!(after_tab.column(), DisplayColumn::new(4));
        assert_eq!(
            map.snapshot()
                .display_point_to_offset(after_tab)
                .expect("显示列应能还原为 tab 后的偏移"),
            ByteOffset::new(1)
        );
    }

    #[test]
    fn projection_map_rejects_out_of_bounds_points_and_invalid_byte_boundaries() {
        let buffer = Buffer::scratch("你".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = DisplayMap::new(buffer.snapshot());

        assert!(
            map.snapshot()
                .buffer_snapshot()
                .position_to_byte(Position::new(Line::ZERO, LogicalColumn::new(2)))
                .is_err()
        );
        assert!(
            map.snapshot()
                .display_point_to_offset(
                    DisplayPoint::new(DisplayRow::new(1), DisplayColumn::ZERO,)
                )
                .is_err()
        );
        assert!(
            map.snapshot()
                .offset_to_display_point(ByteOffset::new(1))
                .is_err()
        );
    }

    #[test]
    fn projection_map_keeps_its_snapshot_version_after_buffer_changes() {
        let mut buffer = Buffer::scratch("a".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = DisplayMap::new(buffer.snapshot());
        let mapped_version = map.snapshot().buffer_snapshot().version();

        buffer
            .edit(
                [Edit::insert(ByteOffset::new(1), "b").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");

        assert_ne!(mapped_version, buffer.version());
        assert_eq!(map.snapshot().buffer_snapshot().version(), mapped_version);
        assert_eq!(
            map.snapshot().buffer_snapshot().len_bytes(),
            ByteOffset::new(1)
        );
        assert!(
            map.snapshot()
                .offset_to_display_point(ByteOffset::new(2))
                .is_err()
        );
    }

    #[test]
    fn folding_changes_display_rows_and_viewport_contents() {
        let buffer = Buffer::scratch(
            "anchor\nhidden one\nhidden two\nafter".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        let before = map.snapshot();
        map.fold_range(
            TextRange::new(ByteOffset::new(6), ByteOffset::new(28)).expect("折叠范围应合法"),
        )
        .expect("折叠应成功");

        assert_eq!(map.snapshot().line_count(), 2);
        assert_eq!(
            map.snapshot()
                .offset_to_display_point(ByteOffset::new("anchor\nhidden ".len()))
                .expect("隐藏位置应能投影")
                .row(),
            DisplayRow::ZERO
        );

        let snapshot = map.snapshot();
        assert_ne!(before.version(), snapshot.version());
        assert_eq!(
            before.buffer_snapshot().version(),
            snapshot.buffer_snapshot().version()
        );
        let mut cursor = snapshot.rows(DisplayRow::ZERO, 8);
        let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[1].kind(), WrapRowKind::Text { .. }));
    }

    #[test]
    fn measuring_folded_rows_uses_tab_projection_rows() {
        let text = "before\nfn folded() {\n  let value = 1;\n}\nafter\n";
        let buffer = Buffer::scratch(text.to_owned(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        let fold_start = text.find('\n').expect("折叠入口行应有换行符");
        let fold_end = text.find("}\n").expect("折叠范围应有闭合行");
        map.fold_range(
            TextRange::new(ByteOffset::new(fold_start), ByteOffset::new(fold_end))
                .expect("折叠范围应合法"),
        )
        .expect("折叠应成功");

        map.measure_rows(DisplayRow::ZERO, map.snapshot().line_count())
            .expect("折叠后的每个显示行都应能完成测量");
    }

    #[test]
    fn folded_bracket_projects_close_to_merged_row() {
        // 回归：折叠后闭合括号保留可见，光标在 `{` 上的括号高亮投影到合并行的真实 `}` 列。
        let buffer = Buffer::scratch(
            "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        // 折叠 fn main：范围 = [行 0 换行符(11), `}`(27))。
        map.fold_range(
            TextRange::new(ByteOffset::new(11), ByteOffset::new(27)).expect("折叠范围应合法"),
        )
        .expect("折叠应成功");
        let snapshot = map.snapshot();

        // 真实 `}` 的字节范围投影到合并行占位符之后的列（anchor 11 字符 + 占位符 1 列 = 12）。
        let projected = snapshot
            .project_text_range(
                TextRange::new(ByteOffset::new(27), ByteOffset::new(28)).expect("`}` 范围应合法"),
            )
            .expect("投影应成功");
        assert_eq!(projected.len(), 1);
        assert_eq!(
            projected[0].start(),
            ProjectedPoint::new(ProjectedLineIndex::new(0), LogicalColumn::new(12))
        );
        assert_eq!(
            projected[0].end(),
            ProjectedPoint::new(ProjectedLineIndex::new(0), LogicalColumn::new(13))
        );

        // 占位符列（11）吸附折叠起点字节；尾段列（12）映射到 close 行字节（`}`）。
        assert_eq!(
            snapshot
                .display_point_to_offset(DisplayPoint::new(
                    DisplayRow::ZERO,
                    DisplayColumn::new(11)
                ))
                .expect("占位符列应可映射"),
            ByteOffset::new(11)
        );
        assert_eq!(
            snapshot
                .display_point_to_offset(DisplayPoint::new(
                    DisplayRow::ZERO,
                    DisplayColumn::new(12)
                ))
                .expect("尾段列应可映射"),
            ByteOffset::new(27)
        );
        assert_eq!(
            snapshot
                .display_point_to_offset_with_bias(
                    DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(11)),
                    FoldBias::Left,
                )
                .expect("占位符左偏置应可映射"),
            ByteOffset::new(11)
        );
        assert_eq!(
            snapshot
                .display_point_to_offset_with_bias(
                    DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(11)),
                    FoldBias::Right,
                )
                .expect("占位符右偏置应可映射到折叠终点"),
            ByteOffset::new(27)
        );
        // 合并行行尾 = close 行内容末尾。
        assert_eq!(
            map.snapshot()
                .end_of_row(ByteOffset::new(11))
                .expect("行尾应可定位"),
            ByteOffset::new(28)
        );
        // 可见字节全偏移 roundtrip（26 是折叠内隐藏字节，投影不可逆）。
        for offset in [0usize, 11, 27, 28, 29, 57] {
            let point = snapshot
                .offset_to_display_point(ByteOffset::new(offset))
                .expect("可见偏移应能映射");
            assert_eq!(
                snapshot
                    .display_point_to_offset(point)
                    .expect("显示点应能还原"),
                ByteOffset::new(offset)
            );
        }
    }

    #[test]
    fn tab_map_invalidates_only_changed_measured_line() {
        let mut buffer = Buffer::scratch("short\nlonger".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        assert_eq!(map.tab_map.measured_lines().count(), 0);
        map.measure_rows(DisplayRow::ZERO, 2)
            .expect("测试显示行应能测量");
        assert_eq!(map.longest_measured_row(), DisplayRow::new(1));
        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(5), " becomes longest").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");
        let outcome = map.sync(buffer.snapshot(), subscription.consume());

        assert_eq!(outcome, ApplyOutcome::Compatible);
        assert_eq!(map.longest_measured_row(), DisplayRow::new(1));
        map.measure_rows(DisplayRow::ZERO, 1)
            .expect("变更行应能按需重新测量");
        assert_eq!(map.longest_measured_row(), DisplayRow::ZERO);
    }

    #[test]
    fn tab_snapshot_advances_when_configuration_changes_without_a_buffer_edit() {
        let mut buffer = Buffer::scratch("\t".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.measure_rows(DisplayRow::ZERO, 1)
            .expect("初始 Tab 行应能测量");
        assert_eq!(
            map.tab_map.measured_lines().next().map(|(_, width)| width),
            Some(DisplayColumn::new(4))
        );

        let before = map.snapshot();
        let subscription = buffer.subscribe();
        let mut config = buffer.config().clone();
        config.tab.tab_width = NonZeroUsize::new(2).expect("测试 Tab 宽度必须非零");
        buffer.set_config(config);
        map.sync(buffer.snapshot(), subscription.consume());

        let after = map.snapshot();
        assert_ne!(before.version(), after.version());
        assert_eq!(map.tab_map.measured_lines().count(), 0);
        map.measure_rows(DisplayRow::ZERO, 1)
            .expect("配置变化后的 Tab 行应能重新测量");
        assert_eq!(
            map.tab_map.measured_lines().next().map(|(_, width)| width),
            Some(DisplayColumn::new(2))
        );
    }

    #[test]
    fn rows_consumes_the_requested_rows() {
        let buffer = Buffer::scratch("a\nb\nc".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = DisplayMap::new(buffer.snapshot());
        let snapshot = map.snapshot();
        let mut cursor = snapshot.rows(DisplayRow::ZERO, snapshot.line_count());
        let mut rows = Vec::new();
        while let Some(row) = cursor.next() {
            rows.push(row);
        }
        assert_eq!(rows.len(), snapshot.line_count());
    }

    #[test]
    fn structural_edit_shifts_tab_measurements_instead_of_clearing_them() {
        let mut buffer = Buffer::scratch("short\nwide".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.measure_rows(DisplayRow::ZERO, 2)
            .expect("测试显示行应能测量");
        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(5), "\nvery very wide").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");

        assert_eq!(
            map.sync(buffer.snapshot(), subscription.consume()),
            ApplyOutcome::Spliced
        );
        // 未受影响的已测行（"wide"）从第 1 行平移到第 2 行，缓存保留；
        // 被编辑的第 0 行失效，重新测量前不参与最长行。
        assert_eq!(map.longest_measured_row(), DisplayRow::new(2));
        map.measure_rows(DisplayRow::new(1), 1)
            .expect("结构编辑后的行应能惰性测量");
        assert_eq!(map.longest_measured_row(), DisplayRow::new(1));
    }

    fn wrap_map(text: &str, width: f32, cx: &TestAppContext) -> DisplayMap {
        let buffer = Buffer::scratch(text.to_owned(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.set_wrap_width(
            Some(px(width)),
            font("Helvetica"),
            px(16.),
            cx.text_system(),
        );
        map
    }

    /// 对每个字符边界做 offset ↔ display point 双向 roundtrip。
    fn assert_offset_roundtrip(map: &DisplayMap) {
        let snapshot = map.snapshot();
        let len = snapshot.buffer_snapshot().len_bytes().get();
        let mut offset = 0;
        while offset < len {
            let point = snapshot
                .offset_to_display_point(ByteOffset::new(offset))
                .expect("合法偏移应能映射");
            assert_eq!(
                snapshot
                    .display_point_to_offset(point)
                    .expect("显示点应能还原"),
                ByteOffset::new(offset),
                "offset {offset} roundtrip 失败"
            );
            offset += snapshot
                .buffer_snapshot()
                .text_for_range(
                    TextRange::new(ByteOffset::new(offset), ByteOffset::new(len))
                        .expect("测试范围应合法"),
                )
                .expect("文本应可读取")
                .chars()
                .next()
                .map_or(1, char::len_utf8);
        }
        let _ = len;
    }

    #[gpui::test]
    fn soft_wrap_splits_wide_lines_into_display_rows(cx: &mut TestAppContext) {
        // 前导空白产生续行缩进。
        let map = wrap_map("    aa bbb cccc ddddd eeee\nshort", 72., cx);
        assert!(map.snapshot().is_wrapped());
        assert!(map.snapshot().line_count() > 2, "宽行应拆成多个显示行");

        let snapshot = map.snapshot();
        let mut cursor = snapshot.rows(DisplayRow::ZERO, map.snapshot().line_count());
        let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
        assert_eq!(rows.len(), map.snapshot().line_count());
        assert_eq!(rows[0].index(), DisplayRow::ZERO);

        // 首段行号从 0 开始，续行片段起点大于 0 且带假空格缩进。
        let WrapRowKind::Text {
            fragment_index,
            byte_range,
            indent,
            ..
        } = rows[1].kind();
        assert_eq!(*fragment_index, 1);
        assert!(*indent > 0, "前导空白应产生续行缩进");
        assert!(byte_range.start > 0, "续行应从行中某字节开始");
    }

    #[gpui::test]
    fn soft_wrap_mixed_commit_message_rows_fit_the_shaped_width(cx: &mut TestAppContext) {
        let message = "修复 SVG 与 Markdown 公式预览的缩放、居中、清晰度、颜色及边界裁剪问题";
        let width = px(420.);
        let map = wrap_map(message, 420., cx);
        let snapshot = map.snapshot();
        let mut cursor = snapshot.rows(DisplayRow::ZERO, snapshot.line_count());
        let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
        let text_system = gpui::WindowTextSystem::new(cx.text_system().clone());
        let font = font("Helvetica");
        let run = gpui::TextRun {
            len: message.len(),
            font,
            ..Default::default()
        };

        for row in &rows {
            let WrapRowKind::Text {
                byte_range,
                indent,
                projected_line,
                ..
            } = row.kind();
            let text = snapshot
                .row_text(*projected_line)
                .expect("显示行文本应可解析");
            let mut rendered = " ".repeat(*indent);
            rendered.push_str(&text.as_ref()[byte_range.clone()]);
            let run = gpui::TextRun {
                len: rendered.len(),
                font: run.font.clone(),
                ..run.clone()
            };
            let shaped = text_system.shape_line(rendered.into(), px(16.), &[run], None);
            assert!(
                shaped.width() <= width,
                "提交信息软换行行宽不能超过统一布局宽度：width={width:?}, shaped={:?}",
                shaped.width()
            );
        }
    }

    #[gpui::test]
    fn soft_wrap_without_leading_whitespace_has_zero_indent(cx: &mut TestAppContext) {
        let map = wrap_map("aa bbb cccc ddddd eeee\nshort", 72., cx);
        let snapshot = map.snapshot();
        let mut cursor = snapshot.rows(DisplayRow::ZERO, map.snapshot().line_count());
        let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
        let WrapRowKind::Text { indent, .. } = rows[1].kind();
        assert_eq!(*indent, 0, "无前导空白的行不应产生缩进");
    }

    #[gpui::test]
    fn soft_wrap_passthrough_when_disabled(cx: &mut TestAppContext) {
        let buffer = Buffer::scratch(
            "aa bbb cccc ddddd eeee\nshort".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.set_wrap_width(None, font("Helvetica"), px(16.), cx.text_system());
        assert!(!map.snapshot().is_wrapped());
        assert_eq!(map.snapshot().line_count(), 2);
        assert_offset_roundtrip(&map);
    }

    #[gpui::test]
    fn soft_wrap_coordinates_roundtrip_through_fragments(cx: &mut TestAppContext) {
        // 含 CJK 与 tab 的行，验证片段内列换算与字节映射一致。
        let map = wrap_map("aa bbb\tccc 你好世界 ddddd eeee\nshort", 72., cx);
        assert_offset_roundtrip(&map);
    }

    #[gpui::test]
    fn soft_wrap_inline_edit_rewraps_affected_line(cx: &mut TestAppContext) {
        let mut buffer = Buffer::scratch(
            "aa bbb cccc ddddd eeee\nshort".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.set_wrap_width(Some(px(72.)), font("Helvetica"), px(16.), cx.text_system());
        let before = map.snapshot();
        let wrapped_rows = before.line_count();

        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(ByteOffset::new("aa bbb ".len()), "xxxx").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");
        map.sync(buffer.snapshot(), subscription.consume());

        let after = map.snapshot();
        assert_ne!(before.version(), after.version());
        assert!(after.line_count() >= wrapped_rows, "编辑后行数应重新计算");
        assert_offset_roundtrip(&map);
    }

    #[gpui::test]
    fn soft_wrap_inline_edit_inside_merged_isomorphic_segment_keeps_line_count(
        cx: &mut TestAppContext,
    ) {
        let text = (0..106)
            .map(|row| format!("let value_{row} = {row};\n"))
            .collect::<String>();
        let mut buffer =
            Buffer::scratch(text, BufferConfig::default()).expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.set_wrap_width(Some(px(800.)), font("Helvetica"), px(16.), cx.text_system());
        let expected_lines = buffer.line_count();

        let subscription = buffer.subscribe();
        let edit_offset = buffer.line_start_byte(Line::new(28)).expect("测试行应存在");
        buffer
            .edit(
                [Edit::insert(edit_offset, "#").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("行内插入 # 应成功");
        map.sync(buffer.snapshot(), subscription.consume());

        assert_eq!(
            map.snapshot().buffer_snapshot().line_count(),
            expected_lines
        );
        assert_eq!(map.snapshot().line_count(), expected_lines);
        assert_offset_roundtrip(&map);
    }

    #[gpui::test]
    fn soft_wrap_structural_edit_rewraps_all_rows(cx: &mut TestAppContext) {
        let mut buffer = Buffer::scratch(
            "aa bbb cccc ddddd eeee\nshort".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.set_wrap_width(Some(px(72.)), font("Helvetica"), px(16.), cx.text_system());

        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(3), "\n").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");
        assert_eq!(
            map.sync(buffer.snapshot(), subscription.consume()),
            ApplyOutcome::Spliced
        );
        assert_offset_roundtrip(&map);
    }

    #[gpui::test]
    fn soft_wrap_equal_row_multi_line_edit_stays_incremental(cx: &mut TestAppContext) {
        // 等行数多行编辑（行数不变、折叠拓扑不变，如行移动/撤销回放）：
        // 不再按"含换行"升级为全量重排，软换行逐行增量重排即可。
        let text = (0..40)
            .map(|row| format!("line number {row} content here\n"))
            .collect::<String>();
        let mut buffer =
            Buffer::scratch(text, BufferConfig::default()).expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.set_wrap_width(Some(px(150.)), font("Helvetica"), px(16.), cx.text_system());
        let expected_rows = map.snapshot().line_count();

        let subscription = buffer.subscribe();
        // 替换 3 行为 3 行更长的内容：行数不变、内容变化，增量路径应覆盖全部受影响行。
        buffer
            .edit(
                [Edit::replace(
                TextRange::new(
                    buffer.line_start_byte(Line::new(5)).expect("测试行应存在"),
                    buffer.line_start_byte(Line::new(8)).expect("测试行应存在"),
                )
                .expect("测试行区间应合法"),
                "replaced line aaaaaaaaaaaaaaaaaaaaa\nreplaced line bbbbbbbbbbbbbbbbbbbbbbb\nreplaced line ccccccccccccccccccccc\n",
                )],
                TransactionMetadata::default(),
            )
            .expect("测试事务应成功");

        assert_eq!(
            map.sync(buffer.snapshot(), subscription.consume()),
            ApplyOutcome::Compatible,
            "等行数多行编辑应走增量路径而非全量重排"
        );
        assert_offset_roundtrip(&map);
        // 变更行变长后软换行显示行数应增加（增量重排确实生效）。
        assert!(
            map.snapshot().line_count() > expected_rows,
            "变长内容应产生更多显示行"
        );
    }

    #[gpui::test]
    fn soft_wrap_with_fold_collapses_hidden_rows(cx: &mut TestAppContext) {
        let buffer = Buffer::scratch(
            "anchor\nhidden one\nhidden two\nafter".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.fold_range(
            TextRange::new(ByteOffset::new(6), ByteOffset::new(28)).expect("折叠范围应合法"),
        )
        .expect("折叠应成功");
        map.set_wrap_width(Some(px(72.)), font("Helvetica"), px(16.), cx.text_system());

        let snapshot = map.snapshot();
        let mut cursor = snapshot.rows(DisplayRow::ZERO, 10);
        let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
        assert_eq!(rows.len(), 2, "折叠后仅剩 anchor 与 after 两行");
        // 折叠隐藏区域的位置映射到 anchor 是现状语义（roundtrip 不可逆），
        // 只对可见文本字节做双向验证。
        for offset in [
            0usize,
            "anchor".len(),
            "anchor\nhidden one\nhidden two\nafter".len() - 1,
        ] {
            let point = snapshot
                .offset_to_display_point(ByteOffset::new(offset))
                .expect("可见偏移应能映射");
            assert_eq!(
                snapshot
                    .display_point_to_offset(point)
                    .expect("显示点应能还原"),
                ByteOffset::new(offset)
            );
        }
    }

    #[gpui::test]
    fn soft_wrap_row_boundaries_follow_fragments(cx: &mut TestAppContext) {
        let buffer = Buffer::scratch(
            "aa bbb cccc ddddd eeee".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.set_wrap_width(Some(px(72.)), font("Helvetica"), px(16.), cx.text_system());
        assert!(map.snapshot().line_count() > 1);

        let snapshot = map.snapshot();
        // 第二行（首个续行）行首 = 片段起点字节，行尾 = 片段终点字节。
        let continuation_offset = snapshot
            .display_point_to_offset(DisplayPoint::new(DisplayRow::new(1), DisplayColumn::ZERO))
            .expect("续行行首应可映射");
        assert_eq!(
            map.snapshot()
                .beginning_of_row(continuation_offset)
                .expect("行首应可定位"),
            continuation_offset
        );
        let end = map
            .snapshot()
            .end_of_row(continuation_offset)
            .expect("行尾应可定位");
        assert!(end.get() > continuation_offset.get(), "行尾应在片段终点");
        assert_eq!(
            snapshot
                .display_point_to_offset(DisplayPoint::new(
                    DisplayRow::new(1),
                    DisplayColumn::new(200),
                ))
                .expect("越界列应钳制到行尾"),
            end
        );
        // 片段终点即下一片段起点（前闭后开）：从终点再行首停在下一片段起点。
        assert_eq!(
            map.snapshot()
                .beginning_of_row(end)
                .expect("行尾再行首应回到片段起点"),
            end
        );
        // 片段中间的任意位置行首都回到片段起点。
        let middle = ByteOffset::new((continuation_offset.get() + end.get()) / 2);
        assert_eq!(
            map.snapshot()
                .beginning_of_row(middle)
                .expect("片段中间行首应回到片段起点"),
            continuation_offset
        );
    }

    #[test]
    fn set_inlays_preserves_line_count_and_projects_text() {
        let mut map = DisplayMap::new(
            Buffer::scratch("ab\ncd".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建")
                .snapshot(),
        );
        set_inlays(
            &mut map,
            vec![Inlay {
                position: ByteOffset::new(1),
                text: ": hint".to_owned(),
            }],
        );
        assert_eq!(map.snapshot().line_count(), 2, "行内提示不占行数");
        let snapshot = map.snapshot();
        let mut cursor = snapshot.rows(DisplayRow::ZERO, 1);
        let row = cursor.next().expect("视口应可读取");
        let WrapRowKind::Text { projected_line, .. } = row.kind();
        assert_eq!(
            snapshot.row_text(*projected_line).unwrap().as_ref(),
            "a: hintb\n"
        );
    }

    #[test]
    fn folded_row_streams_inlays_from_anchor_and_close_tail() {
        let text = "a{\nhidden\n}tail\n";
        let mut map = DisplayMap::new(
            Buffer::scratch(text.to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建")
                .snapshot(),
        );
        let close = text.find('}').expect("测试文本应包含闭合括号");
        set_inlays(
            &mut map,
            vec![
                Inlay {
                    position: ByteOffset::new(1),
                    text: "<anchor>".to_owned(),
                },
                Inlay {
                    position: ByteOffset::new(close + 1),
                    text: "<tail>".to_owned(),
                },
            ],
        );
        map.fold_range(
            TextRange::new(
                ByteOffset::new(text.find('\n').expect("入口行应有换行符")),
                ByteOffset::new(close),
            )
            .expect("折叠范围应合法"),
        )
        .expect("折叠应成功");

        let snapshot = map.snapshot();
        let mut rows = snapshot.chunks(
            DisplayRow::ZERO..DisplayRow::new(1),
            HighlightStyles::default(),
            None,
        );
        let mut rendered = String::new();
        let mut inlay_chunks = 0;
        rows.for_each_row(|event| {
            if let DisplayRowEvent::Text { chunks, .. } = event {
                for chunk in chunks {
                    rendered.push_str(chunk.text);
                    inlay_chunks += usize::from(chunk.is_inlay);
                }
            }
        });
        assert_eq!(rendered, "a<anchor>{…}<tail>tail");
        assert_eq!(
            inlay_chunks, 2,
            "anchor 与 close 尾段的提示都必须只输出一次"
        );
    }

    #[test]
    fn inlay_hit_test_maps_through_projection() {
        let mut map = DisplayMap::new(
            Buffer::scratch("abc\n".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建")
                .snapshot(),
        );
        set_inlays(
            &mut map,
            vec![Inlay {
                position: ByteOffset::new(1),
                text: "XY".to_owned(),
            }],
        );
        let snapshot = map.snapshot();
        // 投影文本 "aXYbc"：'b' 的显示列 3 → 原始偏移 1（锚定后）。
        let offset = snapshot
            .display_point_to_offset(DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(3)))
            .expect("锚定后的字符应映射回原始偏移");
        assert_eq!(offset, ByteOffset::new(1));
        // 注入段内（列 1-2）吸附到锚定后（不可逆）。
        let offset = snapshot
            .display_point_to_offset(DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(1)))
            .expect("注入段内应吸附到锚定后");
        assert_eq!(offset, ByteOffset::new(1));
    }

    #[test]
    fn inlay_changes_trigger_rebuild_but_edits_stay_incremental() {
        let mut buffer = Buffer::scratch("ab\ncd\n".to_owned(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        let subscription = buffer.subscribe();
        set_inlays(
            &mut map,
            vec![Inlay {
                position: ByteOffset::new(1),
                text: "x".to_owned(),
            }],
        );
        // 注入配置变化后，行内编辑仍走增量路径（Compatible）。
        buffer
            .edit(
                [Edit::replace(
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(1)).unwrap(),
                    "AB",
                )],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");
        let outcome = map.sync(buffer.snapshot(), subscription.consume());
        assert_eq!(outcome, ApplyOutcome::Compatible);
        // inlay 锚定是静态偏移（编辑后由数据源更新）：replace [0,1) 后偏移 1 落在 'B' 后。
        let snapshot = map.snapshot();
        let mut cursor = snapshot.rows(DisplayRow::ZERO, 1);
        let row = cursor.next().expect("视口应可读取");
        let WrapRowKind::Text { projected_line, .. } = row.kind();
        assert_eq!(
            snapshot.row_text(*projected_line).unwrap().as_ref(),
            "AxBb\n"
        );
    }
}
