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

mod error;
mod fold_map;
mod tab_map;
mod wrap_map;

use gpui::HighlightStyle;
#[cfg(test)]
use zcv_engine::LineRange;
use zcv_engine::{
    BufferVersion, ByteOffset, DisplayColumn, Line, LogicalColumn, Position, Snapshot,
    TextChangeBatch, TextRange,
};
use zcv_language::HighlightSpan;
use zcv_theme::syntax;

use error::DisplayMapResult;
use fold_map::{ApplyOutcome, FoldMap, LogicalProjection};
pub(crate) use fold_map::{ProjectedLineIndex, ProjectedRange};
use tab_map::TabMap;
pub(crate) use tab_map::byte_for_display_column;
use wrap_map::{WrapMap, WrapSnapshot};
pub(crate) use wrap_map::{WrapViewportRowKind, WrapViewportSlice};

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
    /// 全量语法高亮（Editor 在语法解析完成时经 `DisplayMap::set_highlights` 注入）。
    highlights: std::sync::Arc<[HighlightSpan]>,
    /// 高亮缓存对应的 buffer 版本；与当前 buffer 不一致时渲染侧拒绝使用。
    highlights_version: BufferVersion,
    /// capture 索引 → 样式的预展开表（capture 名表变化时重建）。
    highlight_styles: std::sync::Arc<[HighlightStyle]>,
}

impl DisplaySnapshot {
    pub(super) fn buffer_snapshot(&self) -> &Snapshot {
        self.wrap_snapshot.buffer_snapshot()
    }

    /// 与当前 buffer 版本匹配的可见范围高亮（有序切片，零树遍历）。
    ///
    /// 语法插值推进后缓存版本落后于 buffer，此时返回空（等待下一次解析安装）。
    pub(super) fn highlighted_spans(&self, range: &std::ops::Range<usize>) -> &[HighlightSpan] {
        if self.buffer_snapshot().version() != self.highlights_version {
            return &[];
        }
        let start = self
            .highlights
            .partition_point(|span| span.range.end <= range.start);
        let end = self
            .highlights
            .partition_point(|span| span.range.start < range.end);
        &self.highlights[start..end]
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
    fold_map: FoldMap,
    tab_map: TabMap,
    wrap_map: WrapMap,
    /// 全量语法高亮（Editor 在语法解析完成时注入，对齐 Zed 的 push_highlights）。
    highlights: std::sync::Arc<[HighlightSpan]>,
    /// 高亮缓存对应的 buffer 版本。
    highlights_version: BufferVersion,
    /// capture 名字表（与 `highlight_styles` 的构建输入，变化时重建样式表）。
    capture_names: std::sync::Arc<[std::sync::Arc<str>]>,
    /// capture 索引 → 样式的预展开表（渲染每 run 一次数组索引）。
    highlight_styles: std::sync::Arc<[HighlightStyle]>,
}

impl DisplayMap {
    pub(crate) fn new(snapshot: Snapshot) -> Self {
        let (fold_map, fold_snapshot) = FoldMap::new(snapshot.clone());
        let (tab_map, tab_snapshot) = TabMap::new(fold_snapshot);
        let (wrap_map, _) = WrapMap::new(tab_snapshot);
        let version = fold_map.snapshot().buffer_snapshot().version();
        Self {
            fold_map,
            tab_map,
            wrap_map,
            highlights: std::sync::Arc::from([]),
            highlights_version: version,
            capture_names: std::sync::Arc::from([]),
            highlight_styles: std::sync::Arc::from([]),
        }
    }

    /// 注入全量语法高亮与 capture 样式表（语法解析完成时由 Editor 调用）。
    ///
    /// capture 名字表未变化时复用已展开的样式表，避免每帧重建。
    pub(crate) fn set_highlights(
        &mut self,
        highlights: std::sync::Arc<[HighlightSpan]>,
        version: BufferVersion,
        capture_names: std::sync::Arc<[std::sync::Arc<str>]>,
    ) {
        self.highlights = highlights;
        self.highlights_version = version;
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
            highlights: std::sync::Arc::clone(&self.highlights),
            highlights_version: self.highlights_version,
            highlight_styles: std::sync::Arc::clone(&self.highlight_styles),
        }
    }

    #[cfg(test)]
    pub(crate) fn version(&self) -> BufferVersion {
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
            if let Some(projected) = self
                .fold_map
                .snapshot()
                .projected_line(ProjectedLineIndex::new(row))
                && let Some(line) = projected.kind().text_line()
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
        let (fold_snapshot, fold_edits, outcome) = self.fold_map.read(current_snapshot, &batch);
        let tab_snapshot = self.tab_map.sync(fold_snapshot, &fold_edits);
        self.wrap_map.sync(tab_snapshot, &fold_edits);
        outcome
    }

    #[cfg(test)]
    pub(crate) fn fold_lines(&mut self, line_range: LineRange) -> DisplayMapResult<()> {
        let (fold_snapshot, fold_edits) = self.fold_map.write().fold_lines(line_range)?;
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
    use zcv_engine::{Buffer, BufferConfig, LineRange};

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
        map.fold_lines(LineRange::new(Line::ZERO, Line::new(3)).expect("测试行区间应合法"))
            .expect("折叠应成功");

        assert_eq!(map.line_count(), 3);
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
        assert_eq!(viewport.rows().len(), 3);
        assert!(matches!(
            viewport.rows()[1].kind(),
            WrapViewportRowKind::Placeholder(_)
        ));
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
        } = rows[1].kind()
        else {
            panic!("第二行应为文本行");
        };
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
        let WrapViewportRowKind::Text { indent, .. } = viewport.rows()[1].kind() else {
            panic!("第二行应为文本行");
        };
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
    fn soft_wrap_with_fold_keeps_placeholder_rows(cx: &mut TestAppContext) {
        let buffer = Buffer::scratch(
            "anchor\nhidden one\nhidden two\nafter".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.fold_lines(LineRange::new(Line::ZERO, Line::new(3)).expect("测试行区间应合法"))
            .expect("折叠应成功");
        map.set_wrap_width(Some(px(72.)), font("Helvetica"), px(16.), cx.text_system());

        let snapshot = map.snapshot();
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, 10)
            .expect("显示行视口应可读取");
        assert!(
            viewport
                .rows()
                .iter()
                .any(|row| matches!(row.kind(), WrapViewportRowKind::Placeholder(_))),
            "折叠占位符行应保留"
        );
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
}
