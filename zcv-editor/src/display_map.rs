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

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
use crease_map::{Crease, CreaseId, CreaseMap, CreaseSnapshot};
#[cfg(test)]
pub(crate) use decorations::hunk_rendering;
pub(crate) use decorations::{
    DiffDecorationInput, DiffDecorationSnapshot, DisplayDecorations, SearchDecorationInput,
    SearchDecorationSnapshot, diff_row_for_row, is_hollow_hunk,
};
pub use decorations::{EditorHunk, EditorHunkMarkerKind, EditorHunkPart, HunkControlTarget};
pub(crate) use display_width::DisplayColumn;
use edit::ProjectionEdit;
use error::DisplayMapResult;
pub(crate) use fold_map::{FoldBias, FoldRowSegment, ProjectedLineIndex};
use fold_map::{FoldMap, FoldSnapshot, LogicalProjection};
use gpui::{App, AppContext as _, Bounds, Context, Entity, EventEmitter, HighlightStyle, Pixels};
use tab_map::TabMap;
pub(crate) use tab_map::{byte_for_display_column, display_column_for_byte};
pub(crate) use wrap_map::WrapRowKind;
use wrap_map::{WrapEdit, WrapMap, WrapSnapshot};
use zcv_language::HighlightSpan;
use zcv_multi_buffer::{MultiBuffer, MultiBufferSnapshot, MultiBufferSubscription};
use zcv_text::{
    Anchor, Line, LineRange, LogicalColumn, MovementDirection, MovementUnit, Position,
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
    pub(crate) const ZERO: Self = Self(0);

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

    /// 折叠候选（crease）索引；随组合元数据版本重建，渲染按可见行范围查询。
    pub(crate) fn crease_snapshot(&self) -> &CreaseSnapshot {
        &self.crease_snapshot
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

/// DisplayMap 向订阅者广播的显示管线变化。
#[derive(Clone, Debug)]
pub(crate) struct DisplayMapEvent {
    pub(crate) changes: TextChangeBatch,
}

impl EventEmitter<DisplayMapEvent> for DisplayMap {}

/// 把订阅者独立积累的组合文本批次换算成 Buffer 坐标的投影编辑。
///
/// `DisplayMap` 是唯一文本变更消费者：它把批次换算成组合文本编辑后交给 FoldMap，
/// 其后各层只消费上层编辑，不再回读文本层的 PositionMap。
pub(crate) fn buffer_edits_from_batch(
    batch: &TextChangeBatch,
    old_snapshot: &MultiBufferSnapshot,
    new_snapshot: &MultiBufferSnapshot,
) -> Vec<ProjectionEdit<MultiBufferOffset>> {
    if batch.requires_reset() {
        return vec![ProjectionEdit::new(
            MultiBufferOffset::new(0)..old_snapshot.len_bytes(),
            MultiBufferOffset::new(0)..new_snapshot.len_bytes(),
        )];
    }
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
    folded_buffers: HashSet<PathBuf>,
    /// 当前显示管线的持久派生快照；滚动和普通重绘只克隆快照，不重建 BlockSnapshot。
    snapshot: Option<DisplaySnapshot>,
    /// 组合文本源：DisplayMap 是组合文本变更与同步的唯一持有者。
    multi_buffer: Option<Entity<MultiBuffer>>,
    buffer_subscription: Option<MultiBufferSubscription>,
    /// 宿主注入的 hunk 显示输入（显示装饰领域键之一）。
    editor_hunks: Arc<[EditorHunk]>,
    /// 搜索命中的显示输入（显示装饰领域键之一）。
    search: Option<SearchDecorationInput>,
    /// 折叠候选（crease）索引；按源增量替换，滚动帧按锚点 seek 查询。
    crease_map: CreaseMap,
    /// 每个源当前已物化的折叠候选身份；句柄不变即跳过，不做整份重建。
    source_creases: HashMap<Option<gpui::EntityId>, SourceCreases>,
}

/// 一个源在 `CreaseMap` 中已物化的折叠候选。
#[derive(Debug)]
struct SourceCreases {
    /// 该源折叠候选锚点句柄；句柄变化即表示候选集合变化。
    fold_anchors: Arc<[Range<Anchor>]>,
    ids: Vec<CreaseId>,
}

/// 影响显示快照的全部只读输入摘要。
///
/// 快速路径只比较它：文本版本、元数据版本与 capture 表。
/// 折叠候选与装饰输入分别由元数据事件和 rebuild_decorations 显式驱动，不进入本摘要。
#[derive(PartialEq)]
struct DisplaySyncInputs {
    version: zcv_text::BufferVersion,
    metadata_version: u64,
    capture_names: Arc<[Arc<str>]>,
}

impl DisplaySyncInputs {
    fn of(snapshot: &MultiBufferSnapshot) -> Self {
        Self {
            version: snapshot.version(),
            metadata_version: snapshot.metadata_version(),
            capture_names: snapshot.capture_names(),
        }
    }
}

fn default_tab_width() -> NonZeroUsize {
    NonZeroUsize::new(4).expect("默认 tab 宽度必须大于 0")
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
            source_creases: HashMap::new(),
        };
        this.refresh_snapshot(&wrap_snapshot, &[], cx);
        // 换行层自己拥有后台重排；完成后 DisplayMap 观察并重建 Block 投影。
        cx.observe(&this.wrap_map, |display, _, cx| {
            let wrap_edits = display
                .wrap_map
                .update(cx, |map, _| map.take_edits_since_sync());
            if !wrap_edits.is_empty() {
                let wrap_snapshot = display.wrap_map.read(cx).snapshot().clone();
                display.refresh_snapshot(&wrap_snapshot, &wrap_edits, cx);
            }
            cx.notify();
        })
        .detach();
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
        // 绑定后重建一次装饰，使 diff 与折叠候选立即可见。
        self.refresh_snapshot_from_current(cx);
    }

    /// 消费自上次同步以来的组合文本变化，并推进显示管线。
    ///
    /// 返回本次消费的文本变化，供 Editor 推进它自己的投影派生状态（搜索锚点等）。
    /// 文本编辑与纯元数据变化走同一入口：Fold/Tab/Wrap/Block 逐层消费组合文本编辑。
    pub(crate) fn sync_from_multi_buffer(&mut self, cx: &mut Context<Self>) -> TextChangeBatch {
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
        self.sync(snapshot, changes.clone(), cx);
        changes
    }

    pub(super) fn snapshot(&self) -> DisplaySnapshot {
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
        self.rebuild_decorations(cx);
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
        self.rebuild_decorations(cx);
    }

    /// 用当前 wrap 快照重建显示快照与装饰。
    fn refresh_snapshot_from_current(&mut self, cx: &mut Context<Self>) {
        let wrap_snapshot = self.wrap_map.read(cx).snapshot().clone();
        self.refresh_snapshot(&wrap_snapshot, &[], cx);
    }

    /// 只替换当前快照的装饰层；显示拓扑与版本保持不变。
    ///
    /// 装饰输入（diff、搜索、宿主 hunk）变化走这里，避免无意义地重建 Block 投影。
    fn rebuild_decorations(&mut self, cx: &mut Context<Self>) {
        let Some(mut snapshot) = self.snapshot.take() else {
            return;
        };
        snapshot.decorations = Arc::new(self.build_decorations(&snapshot, cx));
        self.snapshot = Some(snapshot);
        cx.notify();
    }

    /// 按当前显示拓扑和装饰输入投影出一份完整装饰。
    fn build_decorations(&self, snapshot: &DisplaySnapshot, cx: &App) -> DisplayDecorations {
        let (hunks, expanded, old_display_ranges, word_diffs) = match &self.multi_buffer {
            Some(multi_buffer) => {
                let multi_buffer = multi_buffer.read(cx);
                (
                    multi_buffer.diff_hunks().to_vec(),
                    multi_buffer.diff_hunk_expanded(),
                    multi_buffer.diff_hunk_old_ranges().to_vec(),
                    multi_buffer.diff_hunk_word_diffs().to_vec(),
                )
            }
            None => (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        };
        DisplayDecorations::new(
            snapshot,
            DiffDecorationInput {
                hunks: &hunks,
                expanded,
                old_display_ranges: &old_display_ranges,
                word_diffs: &word_diffs,
            },
            self.search.as_ref(),
            Arc::clone(&self.editor_hunks),
        )
    }

    fn refresh_snapshot(
        &mut self,
        wrap_snapshot: &WrapSnapshot,
        wrap_edits: &[WrapEdit],
        cx: &App,
    ) {
        let block_snapshot = Arc::new(self.current_block_snapshot(wrap_snapshot, wrap_edits));
        // 折叠候选按源句柄增量同步：未变化的源不重投影；源集合增删由身份差异自然收敛。
        self.reconcile_source_creases(block_snapshot.wrap_snapshot().buffer_snapshot());
        let mut snapshot = DisplaySnapshot {
            block_snapshot,
            crease_snapshot: self.crease_map.snapshot(),
            decorations: Arc::new(DisplayDecorations::empty()),
        };
        snapshot.decorations = Arc::new(self.build_decorations(&snapshot, cx));
        self.snapshot = Some(snapshot);
    }

    /// 按源增量同步折叠候选：只有候选句柄变化的源重投影并替换其 `CreaseId`，其余源保持不动。
    fn reconcile_source_creases(&mut self, snapshot: &MultiBufferSnapshot) {
        let mut changed = Vec::new();
        let mut seen = HashSet::new();
        for (index, (source_id, fold_anchors)) in snapshot.fold_sources().enumerate() {
            seen.insert(source_id);
            let stale = self
                .source_creases
                .get(&source_id)
                .is_none_or(|state| !Arc::ptr_eq(&state.fold_anchors, fold_anchors));
            if stale {
                changed.push((source_id, index, Arc::clone(fold_anchors)));
            }
        }

        // 退出投影的源：移除其候选身份。
        let mut removed_ids = Vec::new();
        self.source_creases.retain(|source_id, state| {
            if seen.contains(source_id) {
                true
            } else {
                removed_ids.extend(state.ids.iter().copied());
                false
            }
        });

        // 候选集合变化的源：移除旧身份、投影新候选；未变化的源不触碰。
        let mut next_ranges = Vec::new();
        let mut counts = Vec::new();
        for (source_id, index, fold_anchors) in changed {
            if let Some(state) = self.source_creases.remove(&source_id) {
                removed_ids.extend(state.ids.iter().copied());
            }
            let ranges = snapshot
                .fold_ranges_for_source(index)
                .into_iter()
                .map(Crease::simple)
                .collect::<Vec<_>>();
            counts.push((source_id, fold_anchors, ranges.len()));
            next_ranges.extend(ranges);
        }
        self.crease_map.remove(removed_ids, snapshot);
        let ids = self.crease_map.insert(next_ranges, snapshot);
        let mut ids = ids.into_iter();
        for (source_id, fold_anchors, count) in counts {
            let ids = ids.by_ref().take(count).collect::<Vec<_>>();
            self.source_creases
                .insert(source_id, SourceCreases { fold_anchors, ids });
        }
    }

    /// 设置 tab 视觉列宽；变化时重建 tab 与 wrap 投影并刷新当前显示快照。
    pub(crate) fn set_tab_width(&mut self, tab_width: NonZeroUsize, cx: &mut Context<Self>) {
        if self.tab_map.snapshot().tab_width() == tab_width {
            return;
        }
        let fold_snapshot = self.fold_map.snapshot().clone();
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &[], tab_width);
        let (wrap_snapshot, wrap_edits) = self
            .wrap_map
            .update(cx, |map, cx| map.sync(tab_snapshot, &[], cx));
        self.refresh_snapshot(&wrap_snapshot, &wrap_edits, cx);
    }

    pub(crate) fn is_buffer_folded(&self, path: &Path) -> bool {
        self.folded_buffers.contains(path)
    }

    pub(crate) fn set_buffer_folded(
        &mut self,
        path: PathBuf,
        folded: bool,
        cx: &mut Context<Self>,
    ) {
        let changed = if folded {
            self.folded_buffers.insert(path)
        } else {
            self.folded_buffers.remove(&path)
        };
        if changed {
            let wrap_snapshot = self.wrap_map.read(cx).snapshot().clone();
            self.refresh_snapshot(&wrap_snapshot, &[], cx);
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
            self.refresh_snapshot(&wrap_snapshot, &wrap_edits, cx);
        }
        changed
    }

    pub(crate) fn measure_rows(
        &mut self,
        start_row: DisplayRow,
        line_count: usize,
        cx: &App,
    ) -> DisplayMapResult<()> {
        let end = start_row
            .get()
            .saturating_add(line_count)
            .min(self.snapshot().line_count());
        let tab_rows = {
            let block_snapshot = &self
                .snapshot
                .as_ref()
                .expect("DisplayMap 初始化后必须存在显示快照")
                .block_snapshot;
            let wrap_snapshot = self.wrap_map.read(cx).snapshot();
            (start_row.get()..end)
                .filter_map(|display_row| {
                    let wrap_row =
                        block_snapshot.display_row_to_wrap_row(DisplayRow::new(display_row))?;
                    Some(wrap_snapshot.tab_row_for_wrap_row(wrap_row))
                })
                .collect::<Result<Vec<_>, _>>()?
        };
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

    /// 用订阅者独立积累的组合 Patch，把整条显示管线推进到当前 Snapshot。
    ///
    /// 换行层拥有自己的后台重排；这里消费它本次发布的换行编辑并重建 Block 投影。
    pub(crate) fn sync(
        &mut self,
        current_snapshot: impl Into<MultiBufferSnapshot>,
        batch: TextChangeBatch,
        cx: &mut Context<Self>,
    ) {
        let current_snapshot = current_snapshot.into();
        let old_snapshot = self.fold_map.snapshot().buffer_snapshot().clone();
        // 无文本、无语法、无 capture 变化且没有未落地换行重排时不做推进，避免滚动帧重复重建显示拓扑。
        // 影响显示快照的只读输入收敛为一个摘要；后续新增输入必须并入 DisplaySyncInputs。
        if batch.is_empty()
            && DisplaySyncInputs::of(&old_snapshot) == DisplaySyncInputs::of(&current_snapshot)
            && !self.wrap_map.read(cx).is_rewrapping()
        {
            return;
        }
        let buffer_edits = buffer_edits_from_batch(&batch, &old_snapshot, &current_snapshot);
        let (fold_snapshot, fold_edits) = self.fold_map.read(current_snapshot, buffer_edits);
        let tab_width = self.tab_map.snapshot().tab_width();
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &fold_edits, tab_width);
        let (wrap_snapshot, wrap_edits) = self
            .wrap_map
            .update(cx, |map, cx| map.sync(tab_snapshot, &fold_edits, cx));
        self.refresh_snapshot(&wrap_snapshot, &wrap_edits, cx);
    }

    /// 折叠组合锚点范围（入口行行尾换行符 → 闭合括号前；闭合括号保留可见）。
    ///
    /// 端点以组合锚点保存，折叠拓扑在每次同步时按当前快照重新解析。
    pub(crate) fn fold_range(
        &mut self,
        range: Range<MultiBufferAnchor>,
        cx: &mut Context<Self>,
    ) -> DisplayMapResult<()> {
        let (fold_snapshot, fold_edits) = self.fold_map.write().fold(range)?;
        let tab_width = self.tab_map.snapshot().tab_width();
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &fold_edits, tab_width);
        let (wrap_snapshot, wrap_edits) = self
            .wrap_map
            .update(cx, |map, cx| map.sync(tab_snapshot, &fold_edits, cx));
        self.refresh_snapshot(&wrap_snapshot, &wrap_edits, cx);
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
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &fold_edits, tab_width);
        let (wrap_snapshot, wrap_edits) = self
            .wrap_map
            .update(cx, |map, cx| map.sync(tab_snapshot, &fold_edits, cx));
        self.refresh_snapshot(&wrap_snapshot, &wrap_edits, cx);
        Ok(())
    }

    fn current_block_snapshot(
        &self,
        wrap_snapshot: &WrapSnapshot,
        wrap_edits: &[WrapEdit],
    ) -> BlockSnapshot {
        let excerpts = self.fold_map.snapshot().buffer_snapshot().excerpts_arc();
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
        BlockSnapshot::new(wrap_snapshot.clone(), excerpts, &self.folded_buffers)
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, num::NonZeroUsize, path::PathBuf, rc::Rc};

    use gpui::{AppContext, TestAppContext, font, px};
    use zcv_language::LanguageBuffer;
    use zcv_text::{Affinity, Buffer, BufferConfig, Edit, Line, TransactionMetadata};
    use zcv_theme::ThemeChoice;

    use super::*;

    fn display_snapshot(cx: &TestAppContext, map: &Entity<DisplayMap>) -> DisplaySnapshot {
        cx.read_entity(map, |map, _| map.snapshot())
    }

    fn longest_measured_row(cx: &TestAppContext, map: &Entity<DisplayMap>) -> DisplayRow {
        cx.read_entity(map, |map, _| map.longest_measured_row())
    }

    fn measured_lines(
        cx: &TestAppContext,
        map: &Entity<DisplayMap>,
    ) -> std::vec::IntoIter<(Line, DisplayColumn)> {
        cx.read_entity(map, |map, _| {
            map.tab_map.measured_lines().collect::<Vec<_>>()
        })
        .into_iter()
    }

    fn sync(
        cx: &mut TestAppContext,
        map: &Entity<DisplayMap>,
        snapshot: impl Into<MultiBufferSnapshot>,
        batch: TextChangeBatch,
    ) {
        cx.update_entity(map, |map, cx| map.sync(snapshot, batch, cx));
    }

    fn measure_rows(
        cx: &mut TestAppContext,
        map: &Entity<DisplayMap>,
        start_row: DisplayRow,
        line_count: usize,
    ) -> DisplayMapResult<()> {
        cx.update_entity(map, |map, cx| map.measure_rows(start_row, line_count, cx))
    }

    fn fold_range(
        cx: &mut TestAppContext,
        map: &Entity<DisplayMap>,
        start: usize,
        end: usize,
    ) -> DisplayMapResult<()> {
        cx.update_entity(map, |map, cx| {
            let range = {
                let display = map.snapshot();
                let snapshot = display.buffer_snapshot();
                snapshot.anchor_at(MultiBufferOffset::new(start), Affinity::Before)
                    ..snapshot.anchor_at(MultiBufferOffset::new(end), Affinity::After)
            };
            map.fold_range(range, cx)
        })
    }

    fn set_tab_width(cx: &mut TestAppContext, map: &Entity<DisplayMap>, tab_width: NonZeroUsize) {
        cx.update_entity(map, |map, cx| map.set_tab_width(tab_width, cx));
    }

    fn set_wrap_width(
        cx: &mut TestAppContext,
        map: &Entity<DisplayMap>,
        wrap_width: Option<gpui::Pixels>,
        font: gpui::Font,
        font_size: gpui::Pixels,
    ) {
        let text_system = cx.text_system().clone();
        cx.update_entity(map, |map, cx| {
            map.set_wrap_width(wrap_width, font, font_size, &text_system, cx)
        });
    }

    fn apply_test_theme(cx: &mut TestAppContext, id: &'static str) {
        cx.update(|cx| ThemeChoice::Named(id).apply(cx, None));
    }

    #[gpui::test]
    fn display_snapshot_resolves_syntax_styles_from_current_theme(cx: &mut TestAppContext) {
        apply_test_theme(cx, "light");
        let source_buffer = Buffer::from_text("fn main() {}".to_owned(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let source = cx.new(|cx| {
            LanguageBuffer::new(
                source_buffer,
                Some(PathBuf::from("main.rs")),
                std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
                cx,
            )
        });
        cx.run_until_parked();
        let multi_buffer = cx.new(|cx| MultiBuffer::singleton(source, cx));
        let (_, snapshot) =
            cx.update_entity(&multi_buffer, |multi, cx| multi.subscribe_and_snapshot(cx));
        let map = cx.new(|cx| DisplayMap::new(snapshot, cx));

        let snapshot = display_snapshot(cx, &map);
        let styles = cx.update(|app| snapshot.highlight_styles(app));
        assert!(!styles.is_empty(), "语法解析应提供 capture 表");
        let light = styles[0].color;
        apply_test_theme(cx, "dark");
        let dark = cx.update(|app| snapshot.highlight_styles(app))[0].color;

        assert_ne!(light, dark, "同一 DisplayMap 应按当前主题重新派生语法颜色");
    }

    #[gpui::test]
    fn no_op_sync_reuses_the_display_topology_snapshot(cx: &mut TestAppContext) {
        let buffer = Buffer::from_text("paragraph".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        let before = display_snapshot(cx, &map);
        let current = MultiBufferSnapshot::from(buffer.snapshot());

        sync(cx, &map, current, TextChangeBatch::default());

        let after = display_snapshot(cx, &map);
        assert!(
            Arc::ptr_eq(&after.block_snapshot, &before.block_snapshot),
            "无文本与元数据变化的同步不得重建 Block/Fold/Wrap 显示拓扑"
        );
    }

    #[gpui::test]
    fn display_pipeline_receives_the_source_transaction_batch(cx: &mut TestAppContext) {
        let source_buffer = Buffer::from_text("fn main() {}\n".to_owned(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let source = cx.new(|cx| {
            LanguageBuffer::new(
                source_buffer,
                Some(PathBuf::from("main.rs")),
                std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
                cx,
            )
        });
        cx.run_until_parked();

        let multi_buffer = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
        let (projection_subscription, snapshot) =
            cx.update_entity(&multi_buffer, |multi, cx| multi.subscribe_and_snapshot(cx));
        let display = cx.new(|cx| DisplayMap::new(snapshot, cx));
        display.update(cx, |display, cx| {
            display.set_multi_buffer(multi_buffer.clone(), projection_subscription, cx);
        });

        let source_subscription = cx.read_entity(&source, |source, _| source.subscribe());
        let changes = Rc::new(RefCell::new(None));
        let observed = Rc::clone(&changes);
        let _display_subscription = cx.update(|cx| {
            cx.subscribe(&display, move |_, event, _| {
                if !event.changes.is_empty() {
                    *observed.borrow_mut() = Some(event.changes.clone());
                }
            })
        });

        cx.update_entity(&source, |source, cx| {
            source
                .edit(
                    [Edit::insert(MultiBufferOffset::new(3).into(), "async ").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .expect("测试编辑应成功");
        });
        cx.run_until_parked();

        let source_changes = source_subscription.consume();
        let display_changes = changes
            .borrow_mut()
            .take()
            .expect("显示管线应收到组合投影变化");
        assert_eq!(
            display_changes.transaction_id(),
            source_changes.transaction_id()
        );
        assert_eq!(display_changes.patch(), source_changes.patch());

        let reload_subscription = cx.read_entity(&source, |source, _| source.subscribe());
        cx.update_entity(&source, |source, cx| {
            source
                .reset("fn replacement() {}\n".to_owned(), cx)
                .expect("外部重载应成功");
        });
        cx.run_until_parked();

        let source_reload = reload_subscription.consume();
        let display_reload = changes.borrow_mut().take().expect("显示管线应收到重载变化");
        assert!(source_reload.requires_reset());
        assert!(display_reload.requires_reset());
        assert_eq!(
            display_reload.transaction_id(),
            source_reload.transaction_id(),
            "reset 经过语言、组合与显示投影后必须保留源事务身份"
        );
    }

    #[gpui::test]
    fn projection_map_roundtrips_unicode_buffer_points_and_byte_offsets(cx: &mut TestAppContext) {
        let buffer = Buffer::from_text("a你😀\nβ".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        let cases = [
            MultiBufferOffset::new(0),
            MultiBufferOffset::new(1),
            MultiBufferOffset::new(4),
            MultiBufferOffset::new(8),
            MultiBufferOffset::new(9),
            MultiBufferOffset::new(11),
        ];

        for offset in cases {
            let display_point = display_snapshot(cx, &map)
                .offset_to_display_point(offset)
                .expect("合法字节偏移应能映射");
            assert_eq!(
                display_snapshot(cx, &map)
                    .buffer_snapshot()
                    .byte_to_position(
                        display_snapshot(cx, &map)
                            .display_point_to_offset(display_point)
                            .expect("合法显示点应能还原"),
                    )
                    .expect("合法显示点应能还原"),
                display_snapshot(cx, &map)
                    .buffer_snapshot()
                    .byte_to_position(offset)
                    .expect("合法字节偏移应能转换为位置")
            );
            assert_eq!(
                display_snapshot(cx, &map)
                    .offset_to_display_point(offset)
                    .expect("合法字节偏移应能映射"),
                display_point
            );
            assert_eq!(
                display_snapshot(cx, &map)
                    .display_point_to_offset(display_point)
                    .expect("合法 DisplayPoint 应能转回 MultiBufferOffset"),
                offset
            );
        }
    }

    #[gpui::test]
    fn projection_map_uses_display_columns_for_tabs(cx: &mut TestAppContext) {
        let buffer = Buffer::from_text("\tx".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));

        let after_tab = display_snapshot(cx, &map)
            .offset_to_display_point(MultiBufferOffset::new(1))
            .expect("tab 后的偏移应能映射");
        assert_eq!(after_tab.column(), DisplayColumn::new(4));
        assert_eq!(
            display_snapshot(cx, &map)
                .display_point_to_offset(after_tab)
                .expect("显示列应能还原为 tab 后的偏移"),
            MultiBufferOffset::new(1)
        );
    }

    #[gpui::test]
    fn projection_map_rejects_out_of_bounds_points_and_invalid_byte_boundaries(
        cx: &mut TestAppContext,
    ) {
        let buffer = Buffer::from_text("你".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));

        assert!(
            display_snapshot(cx, &map)
                .buffer_snapshot()
                .position_to_byte(Position::new(Line::ZERO, LogicalColumn::new(2)))
                .is_err()
        );
        assert!(
            display_snapshot(cx, &map)
                .display_point_to_offset(
                    DisplayPoint::new(DisplayRow::new(1), DisplayColumn::ZERO,)
                )
                .is_err()
        );
        assert!(
            display_snapshot(cx, &map)
                .offset_to_display_point(MultiBufferOffset::new(1))
                .is_err()
        );
    }

    #[gpui::test]
    fn projection_map_keeps_its_snapshot_version_after_buffer_changes(cx: &mut TestAppContext) {
        let mut buffer = Buffer::from_text("a".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        let mapped_version = display_snapshot(cx, &map).buffer_snapshot().version();

        buffer
            .edit(
                [Edit::insert(MultiBufferOffset::new(1).into(), "b").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");

        assert_ne!(mapped_version, buffer.version());
        assert_eq!(
            display_snapshot(cx, &map).buffer_snapshot().version(),
            mapped_version
        );
        assert_eq!(
            display_snapshot(cx, &map).buffer_snapshot().len_bytes(),
            MultiBufferOffset::new(1)
        );
        assert!(
            display_snapshot(cx, &map)
                .offset_to_display_point(MultiBufferOffset::new(2))
                .is_err()
        );
    }

    #[gpui::test]
    fn folding_changes_display_rows_and_viewport_contents(cx: &mut TestAppContext) {
        let buffer = Buffer::from_text(
            "anchor\nhidden one\nhidden two\nafter".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        let before = display_snapshot(cx, &map);
        fold_range(cx, &map, 6, 28).expect("折叠应成功");

        assert_eq!(display_snapshot(cx, &map).line_count(), 2);
        assert_eq!(
            display_snapshot(cx, &map)
                .offset_to_display_point(MultiBufferOffset::new("anchor\nhidden ".len()))
                .expect("隐藏位置应能投影")
                .row(),
            DisplayRow::ZERO
        );

        let snapshot = display_snapshot(cx, &map);
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

    #[gpui::test]
    fn measuring_folded_rows_uses_tab_projection_rows(cx: &mut TestAppContext) {
        let text = "before\nfn folded() {\n  let value = 1;\n}\nafter\n";
        let buffer = Buffer::from_text(text.to_owned(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        let fold_start = text.find('\n').expect("折叠入口行应有换行符");
        let fold_end = text.find("}\n").expect("折叠范围应有闭合行");
        fold_range(cx, &map, fold_start, fold_end).expect("折叠应成功");

        measure_rows(
            cx,
            &map,
            DisplayRow::ZERO,
            display_snapshot(cx, &map).line_count(),
        )
        .expect("折叠后的每个显示行都应能完成测量");
    }

    #[gpui::test]
    fn folded_bracket_projects_close_to_merged_row(cx: &mut TestAppContext) {
        // 回归：折叠后闭合括号保留可见，光标在 `{` 上的括号高亮投影到合并行的真实 `}` 列。
        let buffer = Buffer::from_text(
            "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        // 折叠 fn main：范围 = [行 0 换行符(11), `}`(27))。
        fold_range(cx, &map, 11, 27).expect("折叠应成功");
        let snapshot = display_snapshot(cx, &map);

        // 真实 `}` 的字节范围投影到合并行占位符之后的列（anchor 11 字符 + 占位符 1 列 = 12）。
        let projected = snapshot
            .project_text_range(
                MultiBufferRange::new(MultiBufferOffset::new(27), MultiBufferOffset::new(28))
                    .expect("`}` 范围应合法"),
            )
            .expect("投影应成功");
        assert_eq!(projected.len(), 1);
        assert_eq!(
            projected[0].start(),
            DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(12))
        );
        assert_eq!(
            projected[0].end(),
            DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(13))
        );

        // 占位符列（11）吸附折叠起点字节；尾段列（12）映射到 close 行字节（`}`）。
        assert_eq!(
            snapshot
                .display_point_to_offset(DisplayPoint::new(
                    DisplayRow::ZERO,
                    DisplayColumn::new(11)
                ))
                .expect("占位符列应可映射"),
            MultiBufferOffset::new(11)
        );
        assert_eq!(
            snapshot
                .display_point_to_offset(DisplayPoint::new(
                    DisplayRow::ZERO,
                    DisplayColumn::new(12)
                ))
                .expect("尾段列应可映射"),
            MultiBufferOffset::new(27)
        );
        assert_eq!(
            snapshot
                .display_point_to_offset_with_bias(
                    DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(11)),
                    FoldBias::Left,
                )
                .expect("占位符左偏置应可映射"),
            MultiBufferOffset::new(11)
        );
        assert_eq!(
            snapshot
                .display_point_to_offset_with_bias(
                    DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(11)),
                    FoldBias::Right,
                )
                .expect("占位符右偏置应可映射到折叠终点"),
            MultiBufferOffset::new(27)
        );
        // 合并行行尾 = close 行内容末尾。
        assert_eq!(
            display_snapshot(cx, &map)
                .end_of_row(MultiBufferOffset::new(11))
                .expect("行尾应可定位"),
            MultiBufferOffset::new(28)
        );
        // 可见字节全偏移 roundtrip（26 是折叠内隐藏字节，投影不可逆）。
        for offset in [0usize, 11, 27, 28, 29, 57] {
            let point = snapshot
                .offset_to_display_point(MultiBufferOffset::new(offset))
                .expect("可见偏移应能映射");
            assert_eq!(
                snapshot
                    .display_point_to_offset(point)
                    .expect("显示点应能还原"),
                MultiBufferOffset::new(offset)
            );
        }
    }

    #[gpui::test]
    fn tab_map_invalidates_only_changed_measured_line(cx: &mut TestAppContext) {
        let mut buffer = Buffer::from_text("short\nlonger".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        assert_eq!(measured_lines(cx, &map).count(), 0);
        measure_rows(cx, &map, DisplayRow::ZERO, 2).expect("测试显示行应能测量");
        assert_eq!(longest_measured_row(cx, &map), DisplayRow::new(1));
        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(MultiBufferOffset::new(5).into(), " becomes longest").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");
        sync(cx, &map, buffer.snapshot(), subscription.consume());
        assert_eq!(longest_measured_row(cx, &map), DisplayRow::new(1));
        measure_rows(cx, &map, DisplayRow::ZERO, 1).expect("变更行应能按需重新测量");
        assert_eq!(longest_measured_row(cx, &map), DisplayRow::ZERO);
    }

    #[gpui::test]
    fn tab_snapshot_advances_when_tab_width_changes_without_a_buffer_edit(cx: &mut TestAppContext) {
        let buffer = Buffer::from_text("\t".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        measure_rows(cx, &map, DisplayRow::ZERO, 1).expect("初始 Tab 行应能测量");
        assert_eq!(
            measured_lines(cx, &map).next().map(|(_, width)| width),
            Some(DisplayColumn::new(4))
        );

        let before = display_snapshot(cx, &map);
        set_tab_width(
            cx,
            &map,
            NonZeroUsize::new(2).expect("测试 Tab 宽度必须非零"),
        );

        let after = display_snapshot(cx, &map);
        assert_ne!(before.version(), after.version());
        assert_eq!(measured_lines(cx, &map).count(), 0);
        measure_rows(cx, &map, DisplayRow::ZERO, 1).expect("配置变化后的 Tab 行应能重新测量");
        assert_eq!(
            measured_lines(cx, &map).next().map(|(_, width)| width),
            Some(DisplayColumn::new(2))
        );
    }

    #[gpui::test]
    fn rows_consumes_the_requested_rows(cx: &mut TestAppContext) {
        let buffer = Buffer::from_text("a\nb\nc".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        let snapshot = display_snapshot(cx, &map);
        let mut cursor = snapshot.rows(DisplayRow::ZERO, snapshot.line_count());
        let mut rows = Vec::new();
        while let Some(row) = cursor.next() {
            rows.push(row);
        }
        assert_eq!(rows.len(), snapshot.line_count());
    }

    #[gpui::test]
    fn structural_edit_shifts_tab_measurements_instead_of_clearing_them(cx: &mut TestAppContext) {
        let mut buffer = Buffer::from_text("short\nwide".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        measure_rows(cx, &map, DisplayRow::ZERO, 2).expect("测试显示行应能测量");
        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(MultiBufferOffset::new(5).into(), "\nvery very wide").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");

        sync(cx, &map, buffer.snapshot(), subscription.consume());
        // 未受影响的已测行（"wide"）从第 1 行平移到第 2 行，缓存保留；
        // 被编辑的第 0 行失效，重新测量前不参与最长行。
        assert_eq!(longest_measured_row(cx, &map), DisplayRow::new(2));
        measure_rows(cx, &map, DisplayRow::new(1), 1).expect("结构编辑后的行应能惰性测量");
        assert_eq!(longest_measured_row(cx, &map), DisplayRow::new(1));
    }

    fn wrap_map(text: &str, width: f32, cx: &mut TestAppContext) -> Entity<DisplayMap> {
        let buffer = Buffer::from_text(text.to_owned(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        set_wrap_width(cx, &map, Some(px(width)), font("Helvetica"), px(16.));
        map
    }

    #[gpui::test]
    fn async_rewrap_settles_after_background_task(cx: &mut TestAppContext) {
        // 大文本 + 软换行：确保重排超出同步时限，走后台任务，再由 run_until_parked 落地。
        let text: String = (0..1_500)
            .map(|row| format!("line {row} 这是一段足够长的中文文本，用来触发软换行与后台重排\n"))
            .collect();
        let mut buffer =
            Buffer::from_text(text, BufferConfig::default()).expect("测试 Buffer 应能创建");
        let snapshot: MultiBufferSnapshot = buffer.snapshot().into();
        let display = cx.new(|cx| {
            let mut display_map = DisplayMap::new(snapshot.clone(), cx);
            display_map.set_wrap_width(
                Some(px(120.)),
                font("Helvetica"),
                px(16.),
                &cx.text_system().clone(),
                cx,
            );
            display_map
        });
        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(MultiBufferOffset::new(0).into(), "新插入的一行\n").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");
        let updated: MultiBufferSnapshot = buffer.snapshot().into();
        let batch = subscription.consume();
        cx.update_entity(&display, |display_map, cx| {
            display_map.sync(updated, batch, cx);
        });
        cx.run_until_parked();

        let line_count = display_snapshot(cx, &display).line_count();
        assert!(
            line_count > 1_500,
            "软换行后显示行数应显著增加，实际 {line_count}"
        );
        // 后台完成后 offset → display point → offset 仍一致。
        cx.read_entity(&display, |display_map, _| {
            assert_offset_roundtrip(display_map)
        });
    }

    /// 对每个字符边界做 offset ↔ display point 双向 roundtrip。
    fn assert_offset_roundtrip(map: &DisplayMap) {
        let snapshot = map.snapshot();
        let len = snapshot.buffer_snapshot().len_bytes().get();
        let mut offset = 0;
        while offset < len {
            let point = snapshot
                .offset_to_display_point(MultiBufferOffset::new(offset))
                .expect("合法偏移应能映射");
            assert_eq!(
                snapshot
                    .display_point_to_offset(point)
                    .expect("显示点应能还原"),
                MultiBufferOffset::new(offset),
                "offset {offset} roundtrip 失败"
            );
            offset += snapshot
                .buffer_snapshot()
                .text_for_range(
                    MultiBufferRange::new(
                        MultiBufferOffset::new(offset),
                        MultiBufferOffset::new(len),
                    )
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
        assert!(display_snapshot(cx, &map).is_wrapped());
        assert!(
            display_snapshot(cx, &map).line_count() > 2,
            "宽行应拆成多个显示行"
        );

        let snapshot = display_snapshot(cx, &map);
        let mut cursor = snapshot.rows(DisplayRow::ZERO, display_snapshot(cx, &map).line_count());
        let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
        assert_eq!(rows.len(), display_snapshot(cx, &map).line_count());
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
        let snapshot = display_snapshot(cx, &map);
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
        let snapshot = display_snapshot(cx, &map);
        let mut cursor = snapshot.rows(DisplayRow::ZERO, display_snapshot(cx, &map).line_count());
        let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
        let WrapRowKind::Text { indent, .. } = rows[1].kind();
        assert_eq!(*indent, 0, "无前导空白的行不应产生缩进");
    }

    #[gpui::test]
    fn soft_wrap_passthrough_when_disabled(cx: &mut TestAppContext) {
        let buffer = Buffer::from_text(
            "aa bbb cccc ddddd eeee\nshort".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        set_wrap_width(cx, &map, None, font("Helvetica"), px(16.));
        assert!(!display_snapshot(cx, &map).is_wrapped());
        assert_eq!(display_snapshot(cx, &map).line_count(), 2);
        cx.read_entity(&map, |m, _| assert_offset_roundtrip(m));
    }

    #[gpui::test]
    fn soft_wrap_coordinates_roundtrip_through_fragments(cx: &mut TestAppContext) {
        // 含 CJK 与 tab 的行，验证片段内列换算与字节映射一致。
        let map = wrap_map("aa bbb\tccc 你好世界 ddddd eeee\nshort", 72., cx);
        cx.read_entity(&map, |m, _| assert_offset_roundtrip(m));
    }

    #[gpui::test]
    fn soft_wrap_inline_edit_rewraps_affected_line(cx: &mut TestAppContext) {
        let mut buffer = Buffer::from_text(
            "aa bbb cccc ddddd eeee\nshort".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        set_wrap_width(cx, &map, Some(px(72.)), font("Helvetica"), px(16.));
        let before = display_snapshot(cx, &map);
        let wrapped_rows = before.line_count();

        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(MultiBufferOffset::new("aa bbb ".len()).into(), "xxxx").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");
        sync(cx, &map, buffer.snapshot(), subscription.consume());

        let after = display_snapshot(cx, &map);
        assert_ne!(before.version(), after.version());
        assert!(after.line_count() >= wrapped_rows, "编辑后行数应重新计算");
        cx.read_entity(&map, |m, _| assert_offset_roundtrip(m));
    }

    #[gpui::test]
    fn soft_wrap_inline_edit_inside_merged_isomorphic_segment_keeps_line_count(
        cx: &mut TestAppContext,
    ) {
        let text = (0..106)
            .map(|row| format!("let value_{row} = {row};\n"))
            .collect::<String>();
        let mut buffer =
            Buffer::from_text(text, BufferConfig::default()).expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        set_wrap_width(cx, &map, Some(px(800.)), font("Helvetica"), px(16.));
        let expected_lines = buffer.line_count();

        let subscription = buffer.subscribe();
        let edit_offset = buffer.line_start_byte(Line::new(28)).expect("测试行应存在");
        buffer
            .edit(
                [Edit::insert(edit_offset, "#").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("行内插入 # 应成功");
        sync(cx, &map, buffer.snapshot(), subscription.consume());

        assert_eq!(
            display_snapshot(cx, &map).buffer_snapshot().line_count(),
            expected_lines
        );
        assert_eq!(display_snapshot(cx, &map).line_count(), expected_lines);
        cx.read_entity(&map, |m, _| assert_offset_roundtrip(m));
    }

    #[gpui::test]
    fn soft_wrap_structural_edit_rewraps_all_rows(cx: &mut TestAppContext) {
        let mut buffer = Buffer::from_text(
            "aa bbb cccc ddddd eeee\nshort".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        set_wrap_width(cx, &map, Some(px(72.)), font("Helvetica"), px(16.));

        let subscription = buffer.subscribe();
        buffer
            .edit(
                [Edit::insert(MultiBufferOffset::new(3).into(), "\n").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");
        sync(cx, &map, buffer.snapshot(), subscription.consume());
        cx.read_entity(&map, |m, _| assert_offset_roundtrip(m));
    }

    #[gpui::test]
    fn soft_wrap_equal_row_multi_line_edit_stays_incremental(cx: &mut TestAppContext) {
        // 等行数多行编辑（行数不变、折叠拓扑不变，如行移动/撤销回放）：
        // 不再按"含换行"升级为全量重排，软换行逐行增量重排即可。
        let text = (0..40)
            .map(|row| format!("line number {row} content here\n"))
            .collect::<String>();
        let mut buffer =
            Buffer::from_text(text, BufferConfig::default()).expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        set_wrap_width(cx, &map, Some(px(150.)), font("Helvetica"), px(16.));
        let expected_rows = display_snapshot(cx, &map).line_count();

        let subscription = buffer.subscribe();
        // 替换 3 行为 3 行更长的内容：行数不变、内容变化，增量路径应覆盖全部受影响行。
        buffer
            .edit(
                [Edit::replace(
                MultiBufferRange::new(
                    buffer.line_start_byte(Line::new(5)).expect("测试行应存在"),
                    buffer.line_start_byte(Line::new(8)).expect("测试行应存在"),
                )
                .expect("测试行区间应合法").into(),
                "replaced line aaaaaaaaaaaaaaaaaaaaa\nreplaced line bbbbbbbbbbbbbbbbbbbbbbb\nreplaced line ccccccccccccccccccccc\n",
                )],
                TransactionMetadata::default(),
            )
            .expect("测试事务应成功");

        sync(cx, &map, buffer.snapshot(), subscription.consume());
        cx.read_entity(&map, |m, _| assert_offset_roundtrip(m));
        // 变更行变长后软换行显示行数应增加（增量重排确实生效）。
        assert!(
            display_snapshot(cx, &map).line_count() > expected_rows,
            "变长内容应产生更多显示行"
        );
    }

    #[gpui::test]
    fn soft_wrap_with_fold_collapses_hidden_rows(cx: &mut TestAppContext) {
        let buffer = Buffer::from_text(
            "anchor\nhidden one\nhidden two\nafter".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        fold_range(cx, &map, 6, 28).expect("折叠应成功");
        set_wrap_width(cx, &map, Some(px(72.)), font("Helvetica"), px(16.));

        let snapshot = display_snapshot(cx, &map);
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
                .offset_to_display_point(MultiBufferOffset::new(offset))
                .expect("可见偏移应能映射");
            assert_eq!(
                snapshot
                    .display_point_to_offset(point)
                    .expect("显示点应能还原"),
                MultiBufferOffset::new(offset)
            );
        }
    }

    #[gpui::test]
    fn soft_wrap_row_boundaries_follow_fragments(cx: &mut TestAppContext) {
        let buffer = Buffer::from_text(
            "aa bbb cccc ddddd eeee".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
        set_wrap_width(cx, &map, Some(px(72.)), font("Helvetica"), px(16.));
        assert!(display_snapshot(cx, &map).line_count() > 1);

        let snapshot = display_snapshot(cx, &map);
        // 第二行（首个续行）行首 = 片段起点字节，行尾 = 片段终点字节。
        let continuation_offset = snapshot
            .display_point_to_offset(DisplayPoint::new(DisplayRow::new(1), DisplayColumn::ZERO))
            .expect("续行行首应可映射");
        assert_eq!(
            display_snapshot(cx, &map)
                .beginning_of_row(continuation_offset)
                .expect("行首应可定位"),
            continuation_offset
        );
        let end = display_snapshot(cx, &map)
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
            display_snapshot(cx, &map)
                .beginning_of_row(end)
                .expect("行尾再行首应回到片段起点"),
            end
        );
        // 片段中间的任意位置行首都回到片段起点。
        let middle = MultiBufferOffset::new((continuation_offset.get() + end.get()) / 2);
        assert_eq!(
            display_snapshot(cx, &map)
                .beginning_of_row(middle)
                .expect("片段中间行首应回到片段起点"),
            continuation_offset
        );
    }
}
