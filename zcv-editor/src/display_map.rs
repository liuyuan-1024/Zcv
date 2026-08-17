//! 决定 Buffer 文本如何映射到 Editor 的显示坐标。
//!
//! DisplayMap 由一组自底向上的变换层组成。当前已实现：
//! - FoldMap：维护折叠范围和折叠后的文本拓扑；
//! - TabMap：在 FoldSnapshot 之上处理硬 Tab 的显示列；
//! - WrapMap：在 TabSnapshot 之上按像素宽度软换行。
//!
//! 每一层都持有自己的 Map 和不可变 Snapshot；
//! 上一层 Snapshot 固化下一层 Snapshot，从而让一次渲染只能看到一条内部一致的显示状态。
//! 后续 InlayMap 与 BlockMap 继续按相同约定接入。

mod chunk;
mod display_width;
mod error;
mod fold_map;
mod inlay_map;
mod line_stream;
mod tab_map;
mod wrap_map;

pub(crate) use chunk::{LineStyles, chunks_to_runs, render_viewport_chunks};
pub(crate) use display_width::DisplayColumn;
use error::DisplayMapResult;
#[cfg(test)]
pub(crate) use fold_map::ProjectedPoint;
use fold_map::{ApplyOutcome, FoldMap, FoldSnapshot, LogicalProjection};
pub(crate) use fold_map::{FoldRowSegment, ProjectedLineIndex, ProjectedRange};
use gpui::HighlightStyle;
pub(crate) use inlay_map::Inlay;
use inlay_map::InlayMap;
use line_stream::LineStream;
pub(crate) use line_stream::{InsertedLines, StreamLineSource};
use tab_map::TabMap;
pub(crate) use tab_map::byte_for_display_column;
use wrap_map::{WrapMap, WrapSnapshot};
pub(crate) use wrap_map::{WrapViewportRowKind, WrapViewportSlice};
use zcv_engine::{
    ByteOffset, Line, LineRange, LogicalColumn, Position, Snapshot, TextChangeBatch, TextRange,
};
use zcv_language::{HighlightSpan, SyntaxSnapshot};
use zcv_theme::syntax;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct BufferPoint {
    line: Line,
    column: LogicalColumn,
}

impl BufferPoint {
    #[cfg(test)]
    pub(crate) const ZERO: Self = Self {
        line: Line::ZERO,
        column: LogicalColumn::ZERO,
    };

    pub(crate) const fn new(line: Line, column: LogicalColumn) -> Self {
        Self { line, column }
    }

    pub(crate) const fn line(self) -> Line {
        self.line
    }

    pub(crate) const fn column(self) -> LogicalColumn {
        self.column
    }

    #[cfg(test)]
    const fn position(self) -> Position {
        Position::new(self.line, self.column)
    }
}

impl From<Position> for BufferPoint {
    fn from(position: Position) -> Self {
        Self::new(position.line(), position.column())
    }
}

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
/// 不会阻塞 Editor 接收后续 Buffer 更新。语法高亮缓存同样以 Arc 携带。
#[derive(Debug, Clone)]
pub(super) struct DisplaySnapshot {
    wrap_snapshot: WrapSnapshot,
    /// 折叠拓扑快照（渲染侧查询折叠状态与合并行段）。
    fold_snapshot: FoldSnapshot,
    /// 语法快照（插值树与 buffer 版本同步；渲染按可见范围懒查询高亮）。
    syntax_snapshot: SyntaxSnapshot,
    /// capture 索引 → 样式的预展开表（capture 名表变化时重建）。
    highlight_styles: std::sync::Arc<[HighlightStyle]>,
}

impl DisplaySnapshot {
    pub(super) fn wrap_snapshot(&self) -> &WrapSnapshot {
        &self.wrap_snapshot
    }

    pub(super) fn buffer_snapshot(&self) -> &Snapshot {
        self.wrap_snapshot.buffer_snapshot()
    }

    /// 从视口 chunk 的真实 buffer 来源查询高亮。
    ///
    /// 和 Zed 的 `BufferChunks -> InlayChunks -> FoldChunks -> TabChunks -> WrapChunks` 顺序一致：语法坐标只存在于最底层 buffer 行。
    /// 软换行片段的投影长度绝不能反推成 buffer 字节范围；
    /// 折叠行则查询其每个有源段对应的真实 buffer 行。
    pub(super) fn highlighted_spans_for_viewport(
        &self,
        viewport: &WrapViewportSlice<'_>,
    ) -> Vec<HighlightSpan> {
        let mut buffer_lines = std::collections::BTreeSet::new();
        let stream = self.fold_snapshot.inlay_snapshot().stream();
        for row in viewport.rows() {
            let WrapViewportRowKind::Text {
                source, segments, ..
            } = row.kind();
            if let Some(segments) = segments {
                for segment in segments {
                    if let fold_map::FoldRowSegmentKind::Text { stream_line, .. } = &segment.kind
                        && let Some(StreamLineSource::Buffer(line)) = stream.source(*stream_line)
                    {
                        buffer_lines.insert(line);
                    }
                }
            } else if let StreamLineSource::Buffer(line) = source {
                buffer_lines.insert(*line);
            }
        }

        let buffer = self.buffer_snapshot();
        let mut spans = Vec::new();
        for line in buffer_lines {
            let Ok(start) = buffer.line_start_byte(Line::new(line)) else {
                continue;
            };
            let end = if line + 1 < buffer.line_count() {
                match buffer.line_start_byte(Line::new(line + 1)) {
                    Ok(end) => end,
                    Err(_) => continue,
                }
            } else {
                buffer.len_bytes()
            };
            spans.extend(
                self.syntax_snapshot
                    .highlights(start.get()..end.get(), buffer),
            );
        }
        spans
    }

    /// capture 索引 → 样式的预展开表（渲染每 run 一次数组索引）。
    pub(super) fn highlight_styles(&self) -> &[HighlightStyle] {
        &self.highlight_styles
    }

    #[cfg(test)]
    pub(super) const fn version(&self) -> u64 {
        self.wrap_snapshot.version()
    }

    pub(super) fn line_count(&self) -> usize {
        self.wrap_snapshot.line_count()
    }

    /// 折叠入口行集合（crease 折叠态与占位符命中判断）。
    pub(super) fn fold_anchor_lines(&self) -> Vec<Line> {
        self.fold_snapshot.fold_anchor_lines()
    }

    /// 覆盖该字节偏移的最外层折叠的隐藏范围（水平移动跨折叠吸附用）。
    pub(super) fn fold_range_covering_offset(
        &self,
        offset: ByteOffset,
    ) -> Option<(ByteOffset, ByteOffset)> {
        self.fold_snapshot.fold_range_covering_offset(offset)
    }

    /// 逻辑行 → 该行首个显示行（wrap 下行首）；行号越界返回 None。
    ///
    /// 组合 position_to_byte + offset_to_display_point（滚动轴 diff marker 用）。
    pub(super) fn line_to_display_row(&self, line: Line) -> Option<DisplayRow> {
        let offset = self
            .buffer_snapshot()
            .position_to_byte(Position::new(line, LogicalColumn::ZERO))
            .ok()?;
        self.offset_to_display_point(offset)
            .ok()
            .map(DisplayPoint::row)
    }

    pub(super) fn is_wrapped(&self) -> bool {
        self.wrap_snapshot.is_wrapped()
    }

    pub(super) fn slice_viewport(
        &self,
        start_row: DisplayRow,
        line_count: usize,
    ) -> DisplayMapResult<WrapViewportSlice<'_>> {
        self.wrap_snapshot.slice_viewport(start_row, line_count)
    }

    pub(super) fn project_text_range(
        &self,
        range: TextRange,
    ) -> DisplayMapResult<Vec<ProjectedRange>> {
        self.wrap_snapshot.project_text_range(range)
    }

    pub(super) fn offset_to_display_point(
        &self,
        offset: ByteOffset,
    ) -> DisplayMapResult<DisplayPoint> {
        self.wrap_snapshot.offset_to_display_point(offset)
    }

    pub(super) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<ByteOffset> {
        self.wrap_snapshot.display_point_to_offset(point)
    }

    pub(super) fn display_to_logical_column(
        &self,
        line: Line,
        column: DisplayColumn,
    ) -> DisplayMapResult<LogicalColumn> {
        self.wrap_snapshot
            .tab_snapshot()
            .display_to_logical_column(line, column)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DisplayMap {
    inlay_map: InlayMap,
    fold_map: FoldMap,
    tab_map: TabMap,
    wrap_map: WrapMap,
    /// 合成行配置（删除块展开的被删除行等外部文本；变化时整链重建）。
    inserted: InsertedLines,
    /// 行内提示配置（inlay 注入；变化时整链重建）。
    inlays: Vec<Inlay>,
    /// 语法快照（Editor 在语法更新时注入；渲染按可见范围懒查询高亮，不再缓存全量 spans）。
    syntax_snapshot: SyntaxSnapshot,
    /// capture 名字表（与 `highlight_styles` 的构建输入，变化时重建样式表）。
    capture_names: std::sync::Arc<[std::sync::Arc<str>]>,
    /// capture 索引 → 样式的预展开表（渲染每 run 一次数组索引）。
    highlight_styles: std::sync::Arc<[HighlightStyle]>,
}

impl DisplayMap {
    pub(crate) fn new(snapshot: Snapshot) -> Self {
        let stream = LineStream::new(snapshot);
        let (inlay_map, inlay_snapshot) = InlayMap::new(stream);
        let (fold_map, fold_snapshot) = FoldMap::new(inlay_snapshot);
        let (tab_map, tab_snapshot) = TabMap::new(fold_snapshot);
        let (wrap_map, _) = WrapMap::new(tab_snapshot);
        let version = fold_map.snapshot().buffer_snapshot().version();
        Self {
            inlay_map,
            fold_map,
            tab_map,
            wrap_map,
            inserted: InsertedLines::new(),
            inlays: Vec::new(),
            syntax_snapshot: SyntaxSnapshot::empty(version),
            capture_names: std::sync::Arc::from([]),
            highlight_styles: std::sync::Arc::from([]),
        }
    }

    /// 注入语法快照与 capture 样式表（语法更新时由 Editor 调用）。
    ///
    /// 渲染按可见范围懒查询高亮；capture 名字表未变化时复用已展开的样式表。
    pub(crate) fn set_syntax_snapshot(&mut self, syntax_snapshot: SyntaxSnapshot) {
        let capture_names = syntax_snapshot.capture_names();
        self.syntax_snapshot = syntax_snapshot;
        if self.capture_names != capture_names {
            self.capture_names = capture_names;
            self.highlight_styles = std::sync::Arc::from(syntax::style_table(&self.capture_names));
        }
    }

    pub(crate) fn buffer_snapshot(&self) -> &Snapshot {
        self.fold_map.snapshot().buffer_snapshot()
    }

    pub(super) fn snapshot(&self) -> DisplaySnapshot {
        DisplaySnapshot {
            wrap_snapshot: self.wrap_map.snapshot().clone(),
            fold_snapshot: self.fold_map.snapshot().clone(),
            syntax_snapshot: self.syntax_snapshot.clone(),
            highlight_styles: std::sync::Arc::clone(&self.highlight_styles),
        }
    }

    #[cfg(test)]
    pub(crate) fn version(&self) -> zcv_engine::BufferVersion {
        self.fold_map.snapshot().buffer_snapshot().version()
    }

    pub(crate) fn line_count(&self) -> usize {
        self.wrap_map.snapshot().line_count()
    }

    pub(crate) fn is_wrapped(&self) -> bool {
        self.wrap_map.snapshot().is_wrapped()
    }

    /// 设置软换行宽度与字体；宽度/字体变化时内部重建，返回是否发生变化。
    pub(crate) fn set_wrap_width(
        &mut self,
        wrap_width: Option<gpui::Pixels>,
        font: gpui::Font,
        font_size: gpui::Pixels,
        text_system: &std::sync::Arc<gpui::TextSystem>,
    ) -> bool {
        self.wrap_map
            .set_wrap_width(wrap_width, font, font_size, text_system.clone())
    }

    pub(crate) fn measure_rows(
        &mut self,
        start_row: DisplayRow,
        line_count: usize,
    ) -> DisplayMapResult<()> {
        let end = start_row
            .get()
            .saturating_add(line_count)
            .min(self.fold_map.snapshot().line_count());
        for row in start_row.get()..end {
            if let Some(line) = self
                .fold_map
                .snapshot()
                .projected_line_kind(ProjectedLineIndex::new(row))
            {
                self.tab_map.measure_line(line.logical_line())?;
            }
        }
        Ok(())
    }

    pub(crate) fn longest_measured_row(&self) -> DisplayRow {
        self.tab_map
            .measured_lines()
            .filter_map(|(line, width)| {
                match self.fold_map.snapshot().logical_to_projected(line).ok()? {
                    LogicalProjection::Visible(row) => Some((row, width)),
                    LogicalProjection::Hidden { .. } => None,
                }
            })
            .max_by_key(|(_, width)| *width)
            .map(|(row, _)| DisplayRow::from(row))
            .unwrap_or(DisplayRow::ZERO)
    }

    /// 用订阅者独立积累的组合 Patch，把整条显示管线直接推进到当前 Snapshot。
    pub(crate) fn sync(
        &mut self,
        current_snapshot: Snapshot,
        batch: TextChangeBatch,
    ) -> ApplyOutcome {
        let mut stream = LineStream::new(current_snapshot);
        // 空配置不注入：避免每次 sync 都推进合成行版本导致增量路径失效。
        if !self.inserted.is_empty() {
            stream.set_inserted(self.inserted.clone());
        }
        let (inlay_snapshot, _) = self.inlay_map.read(stream, self.inlays.clone());
        let (fold_snapshot, fold_edits, outcome) = self.fold_map.read(inlay_snapshot, &batch);
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &fold_edits);
        self.wrap_map.sync(tab_snapshot, &fold_edits);
        outcome
    }

    /// 替换合成行配置（删除块展开的被删除行等）并推进整条管线。
    pub(crate) fn set_inserted(&mut self, inserted: InsertedLines) {
        if self.inserted == inserted {
            return;
        }
        self.inserted = inserted;
        // 重建输入流（含新合成行）→ inlay → fold → tab → wrap 逐层推进。
        let fold_snapshot = self.fold_map.snapshot().clone();
        let mut stream = fold_snapshot.stream().clone();
        stream.set_inserted(self.inserted.clone());
        let (inlay_snapshot, _) = self.inlay_map.read(stream, self.inlays.clone());
        let (fold_snapshot, fold_edits, _) = self
            .fold_map
            .read(inlay_snapshot, &TextChangeBatch::default());
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &fold_edits);
        self.wrap_map.sync(tab_snapshot, &fold_edits);
    }

    /// 替换行内提示配置并推进整条管线。
    ///
    /// Inlay hints 显示为规划中能力（对齐 Zed：LSP inlay hints 注入行内提示），当前没有 UI 入口调用本方法，渲染端也未消费 Inlay 数据；
    /// 管线保留以支持后续接入，`dead_code` 警告属于已知的预留例外。
    pub(crate) fn set_inlays(&mut self, inlays: Vec<Inlay>) {
        if self.inlays == inlays {
            return;
        }
        self.inlays = inlays;
        let fold_snapshot = self.fold_map.snapshot().clone();
        let stream = fold_snapshot.stream().clone();
        let (inlay_snapshot, _) = self.inlay_map.read(stream, self.inlays.clone());
        let (fold_snapshot, fold_edits, _) = self
            .fold_map
            .read(inlay_snapshot, &TextChangeBatch::default());
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &fold_edits);
        self.wrap_map.sync(tab_snapshot, &fold_edits);
    }

    /// 折叠字节范围（入口行行尾换行符 → 闭合括号前；闭合括号保留可见）。
    pub(crate) fn fold_range(&mut self, range: TextRange) -> DisplayMapResult<()> {
        let (fold_snapshot, fold_edits) = self.fold_map.write().fold(range)?;
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &fold_edits);
        self.wrap_map.sync(tab_snapshot, &fold_edits);
        Ok(())
    }

    /// 展开与行范围交叠的全部折叠（半开区间）。
    pub(crate) fn unfold_lines(&mut self, line_range: LineRange) -> DisplayMapResult<()> {
        let (fold_snapshot, fold_edits) = self.fold_map.write().unfold_lines(line_range)?;
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &fold_edits);
        self.wrap_map.sync(tab_snapshot, &fold_edits);
        Ok(())
    }

    pub(crate) fn offset_to_display_point(
        &self,
        offset: ByteOffset,
    ) -> DisplayMapResult<DisplayPoint> {
        self.wrap_map.snapshot().offset_to_display_point(offset)
    }

    pub(crate) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<ByteOffset> {
        self.wrap_map.snapshot().display_point_to_offset(point)
    }

    pub(crate) fn beginning_of_row(&self, offset: ByteOffset) -> DisplayMapResult<ByteOffset> {
        self.wrap_map.snapshot().beginning_of_row(offset)
    }

    pub(crate) fn end_of_row(&self, offset: ByteOffset) -> DisplayMapResult<ByteOffset> {
        self.wrap_map.snapshot().end_of_row(offset)
    }

    #[cfg(test)]
    pub(crate) fn buffer_point_to_display_point(
        &self,
        point: BufferPoint,
    ) -> DisplayMapResult<DisplayPoint> {
        let offset = self.buffer_snapshot().position_to_byte(point.position())?;
        self.offset_to_display_point(offset)
    }

    #[cfg(test)]
    pub(crate) fn display_point_to_buffer_point(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<BufferPoint> {
        let offset = self.display_point_to_offset(point)?;
        Ok(self
            .buffer_snapshot()
            .byte_to_position(offset)
            .map(BufferPoint::from)?)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use gpui::{TestAppContext, font, px};
    use zcv_engine::{Buffer, BufferConfig, Edit, Line, TextRange, Transaction};

    use super::fold_map::ProjectedPoint;
    use super::line_stream::InsertedLines;
    use super::*;

    #[test]
    fn projection_map_roundtrips_unicode_buffer_points_and_byte_offsets() {
        let buffer = Buffer::scratch("a你😀\nβ".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = DisplayMap::new(buffer.snapshot());
        let cases = [
            (ByteOffset::new(0), BufferPoint::ZERO),
            (
                ByteOffset::new(1),
                BufferPoint::new(Line::ZERO, LogicalColumn::new(1)),
            ),
            (
                ByteOffset::new(4),
                BufferPoint::new(Line::ZERO, LogicalColumn::new(2)),
            ),
            (
                ByteOffset::new(8),
                BufferPoint::new(Line::ZERO, LogicalColumn::new(3)),
            ),
            (
                ByteOffset::new(9),
                BufferPoint::new(Line::new(1), LogicalColumn::ZERO),
            ),
            (
                ByteOffset::new(11),
                BufferPoint::new(Line::new(1), LogicalColumn::new(1)),
            ),
        ];

        for (offset, buffer_point) in cases {
            let display_point = map
                .buffer_point_to_display_point(buffer_point)
                .expect("合法 BufferPoint 应能映射");
            assert_eq!(
                map.display_point_to_buffer_point(display_point)
                    .expect("合法 DisplayPoint 应能还原"),
                buffer_point
            );
            assert_eq!(
                map.offset_to_display_point(offset)
                    .expect("合法 ByteOffset 应能映射"),
                display_point
            );
            assert_eq!(
                map.display_point_to_offset(display_point)
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
            .offset_to_display_point(ByteOffset::new(1))
            .expect("tab 后的偏移应能映射");
        assert_eq!(after_tab.column(), DisplayColumn::new(4));
        assert_eq!(
            map.display_point_to_offset(after_tab)
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
            map.buffer_point_to_display_point(BufferPoint::new(Line::ZERO, LogicalColumn::new(2),))
                .is_err()
        );
        assert!(
            map.display_point_to_buffer_point(DisplayPoint::new(
                DisplayRow::new(1),
                DisplayColumn::ZERO,
            ))
            .is_err()
        );
        assert!(map.offset_to_display_point(ByteOffset::new(1)).is_err());
    }

    #[test]
    fn projection_map_keeps_its_snapshot_version_after_buffer_changes() {
        let mut buffer = Buffer::scratch("a".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let map = DisplayMap::new(buffer.snapshot());
        let mapped_version = map.version();

        buffer
            .insert(ByteOffset::new(1), "b")
            .expect("测试编辑应成功");

        assert_ne!(mapped_version, buffer.version());
        assert_eq!(map.version(), mapped_version);
        assert_eq!(map.buffer_snapshot().len_bytes(), ByteOffset::new(1));
        assert!(map.offset_to_display_point(ByteOffset::new(2)).is_err());
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

        assert_eq!(map.line_count(), 2);
        assert_eq!(
            map.offset_to_display_point(ByteOffset::new("anchor\nhidden ".len()))
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
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, 8)
            .expect("投影视口应可读取");
        assert_eq!(viewport.rows().len(), 2);
        assert!(matches!(
            viewport.rows()[1].kind(),
            WrapViewportRowKind::Text { .. }
        ));
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
        // 合并行行尾 = close 行内容末尾。
        assert_eq!(
            map.end_of_row(ByteOffset::new(11)).expect("行尾应可定位"),
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
            .insert(ByteOffset::new(5), " becomes longest")
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
    fn structural_edit_clears_tab_measurements_until_rows_are_requested_again() {
        let mut buffer = Buffer::scratch("short\nwide".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.measure_rows(DisplayRow::ZERO, 2)
            .expect("测试显示行应能测量");
        let subscription = buffer.subscribe();
        buffer
            .insert(ByteOffset::new(5), "\nvery very wide")
            .expect("测试编辑应成功");

        assert_eq!(
            map.sync(buffer.snapshot(), subscription.consume()),
            ApplyOutcome::Spliced
        );
        assert_eq!(map.longest_measured_row(), DisplayRow::ZERO);
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
                .slice_text(
                    zcv_engine::TextRange::new(ByteOffset::new(offset), ByteOffset::new(len))
                        .expect("测试范围应合法"),
                )
                .expect("文本应可读取")
                .as_str()
                .chars()
                .next()
                .map_or(1, char::len_utf8);
        }
        let _ = len;
    }

    #[gpui::test]
    fn soft_wrap_splits_wide_lines_into_display_rows(cx: &mut TestAppContext) {
        // 前导空白产生续行缩进（对齐 Zed 的 Boundary.next_indent 语义）。
        let map = wrap_map("    aa bbb cccc ddddd eeee\nshort", 72., cx);
        assert!(map.is_wrapped());
        assert!(map.line_count() > 2, "宽行应拆成多个显示行");

        let snapshot = map.snapshot();
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, map.line_count())
            .expect("显示行视口应可读取");
        let rows = viewport.rows();
        assert_eq!(rows.len(), map.line_count());
        assert_eq!(rows[0].index(), DisplayRow::ZERO);

        // 首段行号从 0 开始，续行片段起点大于 0 且带假空格缩进。
        let WrapViewportRowKind::Text {
            fragment_index,
            byte_range,
            indent,
            column_base,
            ..
        } = rows[1].kind();
        assert_eq!(*fragment_index, 1);
        assert!(*indent > 0, "前导空白应产生续行缩进");
        assert!(byte_range.start > 0, "续行应从行中某字节开始");
        assert!(*column_base > 0, "续行片段起始字符列应大于 0");
    }

    #[gpui::test]
    fn soft_wrap_without_leading_whitespace_has_zero_indent(cx: &mut TestAppContext) {
        let map = wrap_map("aa bbb cccc ddddd eeee\nshort", 72., cx);
        let snapshot = map.snapshot();
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, map.line_count())
            .expect("显示行视口应可读取");
        let WrapViewportRowKind::Text { indent, .. } = viewport.rows()[1].kind();
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
        assert!(!map.is_wrapped());
        assert_eq!(map.line_count(), 2);
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
            .insert(ByteOffset::new("aa bbb ".len()), "xxxx")
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
        buffer.insert(edit_offset, "#").expect("行内插入 # 应成功");
        map.sync(buffer.snapshot(), subscription.consume());

        assert_eq!(map.buffer_snapshot().line_count(), expected_lines);
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
            .insert(ByteOffset::new(3), "\n")
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
        let transaction = Transaction::from_edits(
            buffer.version(),
            vec![Edit::replace(
                TextRange::new(
                    buffer.line_start_byte(Line::new(5)).expect("测试行应存在"),
                    buffer.line_start_byte(Line::new(8)).expect("测试行应存在"),
                )
                .expect("测试行区间应合法"),
                "replaced line aaaaaaaaaaaaaaaaaaaaa\nreplaced line bbbbbbbbbbbbbbbbbbbbbbb\nreplaced line ccccccccccccccccccccc\n",
            )],
        )
        .expect("测试事务应合法");
        buffer
            .apply_transaction(transaction)
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
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, 10)
            .expect("显示行视口应可读取");
        assert_eq!(viewport.rows().len(), 2, "折叠后仅剩 anchor 与 after 两行");
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
        assert!(map.line_count() > 1);

        let snapshot = map.snapshot();
        // 第二行（首个续行）行首 = 片段起点字节，行尾 = 片段终点字节。
        let continuation_offset = snapshot
            .display_point_to_offset(DisplayPoint::new(DisplayRow::new(1), DisplayColumn::ZERO))
            .expect("续行行首应可映射");
        assert_eq!(
            map.beginning_of_row(continuation_offset)
                .expect("行首应可定位"),
            continuation_offset
        );
        let end = map.end_of_row(continuation_offset).expect("行尾应可定位");
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
            map.beginning_of_row(end).expect("行尾再行首应回到片段起点"),
            end
        );
        // 片段中间的任意位置行首都回到片段起点。
        let middle = ByteOffset::new((continuation_offset.get() + end.get()) / 2);
        assert_eq!(
            map.beginning_of_row(middle)
                .expect("片段中间行首应回到片段起点"),
            continuation_offset
        );
    }

    /// 构造带合成行的 DisplayMap（锚定 buffer 逻辑行 → 其后插入的文本行）。
    fn map_with_inserted(text: &str, inserted: InsertedLines) -> DisplayMap {
        let buffer = Buffer::scratch(text.to_owned(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        let fold_snapshot = map.fold_map.snapshot().clone();
        let mut stream = fold_snapshot.stream().clone();
        stream.set_inserted(inserted);
        let (inlay_snapshot, _) = map.inlay_map.read(stream, map.inlays.clone());
        let (fold_snapshot, fold_edits, _) = map
            .fold_map
            .read(inlay_snapshot, &TextChangeBatch::default());
        let tab_snapshot = map.tab_map.sync(fold_snapshot, &fold_edits);
        map.wrap_map.sync(tab_snapshot, &fold_edits);
        map
    }

    #[test]
    fn inserted_lines_extend_stream_and_interleave_with_buffer_rows() {
        let map = map_with_inserted(
            "a\nb\nc",
            InsertedLines::from([(Line::ZERO, vec![std::sync::Arc::from("DEL")])]),
        );
        assert_eq!(map.line_count(), 4, "3 个 buffer 行 + 1 个合成行");

        let snapshot = map.snapshot();
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, 4)
            .expect("视口应可读取");
        let rows = viewport.rows();
        assert_eq!(rows.len(), 4);
        // 行序交错：buffer 行 0 → 合成行（锚定行 0 之后）→ buffer 行 1/2。
        let WrapViewportRowKind::Text { text, .. } = rows[0].kind();
        assert_eq!(text.as_ref(), "a\n");
        match rows[1].kind() {
            WrapViewportRowKind::Text {
                source: StreamLineSource::Inserted { .. },
                text,
                ..
            } => assert_eq!(text.as_ref(), "DEL"),
            other => panic!("行 1 应为合成行，实际 {other:?}"),
        }
        let WrapViewportRowKind::Text { text, .. } = rows[2].kind();
        assert_eq!(text.as_ref(), "b\n");
    }

    #[test]
    fn inserted_line_maps_to_anchor_byte_offset() {
        let map = map_with_inserted(
            "a\nb",
            InsertedLines::from([(Line::ZERO, vec![std::sync::Arc::from("DEL")])]),
        );
        let snapshot = map.snapshot();
        // 合成行无 buffer 坐标，映射到锚定行（行 0）行首。
        let offset = snapshot
            .display_point_to_offset(DisplayPoint::new(DisplayRow::new(1), DisplayColumn::ZERO))
            .expect("合成行应映射到锚定行行首");
        assert_eq!(offset, ByteOffset::new(0));
    }

    #[gpui::test]
    fn inserted_lines_participate_in_soft_wrap(cx: &mut TestAppContext) {
        let mut map = map_with_inserted(
            "short",
            InsertedLines::from([(
                Line::ZERO,
                vec![std::sync::Arc::from("aaaa bbbb cccc dddd eeee")],
            )]),
        );
        map.set_wrap_width(Some(px(72.)), font("Helvetica"), px(16.), cx.text_system());
        assert!(
            map.line_count() > 2,
            "超宽合成行应像普通行一样软换行拆成多个显示行"
        );
    }

    #[test]
    fn inserted_lines_inside_fold_range_are_collapsed() {
        // 阶段 1 核心：合成行在 fold 输入（文本流层），可被折叠区间一起收起。
        // 合成行插在锚定行（行 2）之前：折叠 0..3 隐藏行 1-2（首行保留），块随行 2 一起收起。
        let mut map = map_with_inserted(
            "a\nb\nc\nd",
            InsertedLines::from([(
                Line::new(2),
                vec![std::sync::Arc::from("DEL1"), std::sync::Arc::from("DEL2")],
            )]),
        );
        assert_eq!(map.line_count(), 6, "4 个 buffer 行 + 2 个合成行");

        // 折叠 buffer 行 0..3（隐藏行 1-2，含块锚定行 2）。
        let (fold_snapshot, fold_edits) = map
            .fold_map
            .write()
            .fold(TextRange::new(ByteOffset::new(1), ByteOffset::new(5)).unwrap())
            .expect("折叠应成功");
        let tab_snapshot = map.tab_map.sync(fold_snapshot, &fold_edits);
        map.wrap_map.sync(tab_snapshot, &fold_edits);

        // 显示行 = 行 0（合并行）+ 行 3。
        assert_eq!(map.line_count(), 2, "6 个流行 - 4 个隐藏");

        let snapshot = map.snapshot();
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, 2)
            .expect("视口应可读取");
        let rows = viewport.rows();
        let WrapViewportRowKind::Text { text, .. } = rows[0].kind();
        // 合并行：anchor 文本 + 占位符。
        assert_eq!(text.as_ref(), "a…\n");
        let WrapViewportRowKind::Text { text, .. } = rows[1].kind();
        assert_eq!(text.as_ref(), "d");
    }

    #[test]
    fn set_inlays_preserves_line_count_and_projects_text() {
        let mut map = DisplayMap::new(
            Buffer::scratch("ab\ncd".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建")
                .snapshot(),
        );
        map.set_inlays(vec![Inlay {
            position: ByteOffset::new(1),
            text: ": hint".to_owned(),
        }]);
        assert_eq!(map.line_count(), 2, "行内提示不占行数");
        let snapshot = map.snapshot();
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, 1)
            .expect("视口应可读取");
        let WrapViewportRowKind::Text { text, .. } = viewport.rows()[0].kind();
        assert_eq!(text.as_ref(), "a: hintb\n");
    }

    #[test]
    fn inlay_hit_test_maps_through_projection() {
        let mut map = DisplayMap::new(
            Buffer::scratch("abc\n".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建")
                .snapshot(),
        );
        map.set_inlays(vec![Inlay {
            position: ByteOffset::new(1),
            text: "XY".to_owned(),
        }]);
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
        map.set_inlays(vec![Inlay {
            position: ByteOffset::new(1),
            text: "x".to_owned(),
        }]);
        // 注入配置变化后，行内编辑仍走增量路径（Compatible）。
        buffer
            .replace(
                TextRange::new(ByteOffset::ZERO, ByteOffset::new(1)).unwrap(),
                "AB",
            )
            .expect("测试编辑应成功");
        let outcome = map.sync(buffer.snapshot(), subscription.consume());
        assert_eq!(outcome, ApplyOutcome::Compatible);
        // inlay 锚定是静态偏移（编辑后由数据源更新）：replace [0,1) 后偏移 1 落在 'B' 后。
        let snapshot = map.snapshot();
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, 1)
            .expect("视口应可读取");
        let WrapViewportRowKind::Text { text, .. } = viewport.rows()[0].kind();
        assert_eq!(text.as_ref(), "AxBb\n");
    }

    #[test]
    fn empty_inserted_lines_match_plain_map() {
        // 回归：无合成行时消费链与现状逐行等价。
        let plain = DisplayMap::new(
            Buffer::scratch("a\nb\nc".to_string(), BufferConfig::default())
                .expect("测试 Buffer 应能创建")
                .snapshot(),
        );
        let with_inserted = map_with_inserted("a\nb\nc", InsertedLines::new());
        assert_eq!(plain.line_count(), with_inserted.line_count());
        let plain_snapshot = plain.snapshot();
        let with_snapshot = with_inserted.snapshot();
        for row in 0..plain.line_count() {
            let plain_row = plain_snapshot
                .slice_viewport(DisplayRow::new(row), 1)
                .expect("视口应可读取")
                .rows()
                .first()
                .expect("应有行")
                .clone();
            let with_row = with_snapshot
                .slice_viewport(DisplayRow::new(row), 1)
                .expect("视口应可读取")
                .rows()
                .first()
                .expect("应有行")
                .clone();
            assert_eq!(plain_row, with_row, "行 {row} 应等价");
        }
    }
}
