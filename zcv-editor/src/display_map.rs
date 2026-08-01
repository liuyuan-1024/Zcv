//! 决定 Buffer 文本如何映射到 Editor 的显示坐标。
//!
//! DisplayMap 由一组自底向上的变换层组成。当前已实现：
//! - FoldMap：维护折叠范围和折叠后的文本拓扑；
//! - TabMap：在 FoldSnapshot 之上处理硬 Tab 的显示列。
//!
//! 每一层都持有自己的 Map 和不可变 Snapshot；
//! 上一层 Snapshot 固化下一层 Snapshot，从而让一次渲染只能看到一条内部一致的显示状态。
//! 后续 InlayMap、WrapMap 与 BlockMap 继续按相同约定接入。

mod error;
mod fold_map;
mod tab_map;

use zcv_engine::{
    ByteOffset, DisplayColumn, Line, LineRange, LogicalColumn, Position, Snapshot, TextChangeBatch,
    TextRange,
};

#[cfg(test)]
use zcv_engine::BufferVersion;

use error::DisplayMapResult;
use fold_map::{
    ApplyOutcome, FoldMap, FoldSnapshot, LogicalPoint, LogicalPointProjection, ProjectedPoint,
    ProjectedPointMapping, ProjectedViewport, ProjectedViewportSlice,
};
pub(crate) use fold_map::{ProjectedLineIndex, ProjectedRange, ProjectedViewportRowKind};
use tab_map::{TabMap, TabSnapshot, display_column_to_byte};

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

    const fn projected(self) -> ProjectedLineIndex {
        ProjectedLineIndex::new(self.0)
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
/// FoldSnapshot 与 TabSnapshot 都是低成本克隆；渲染持有此值时不会阻塞 Editor
/// 接收后续 Buffer 更新。
#[derive(Debug, Clone)]
pub(super) struct DisplaySnapshot {
    tab_snapshot: TabSnapshot,
}

impl DisplaySnapshot {
    pub(super) fn tab_snapshot(&self) -> &TabSnapshot {
        &self.tab_snapshot
    }

    pub(super) fn fold_snapshot(&self) -> &FoldSnapshot {
        self.tab_snapshot.fold_snapshot()
    }

    pub(super) fn buffer_snapshot(&self) -> &Snapshot {
        self.tab_snapshot.buffer_snapshot()
    }

    #[cfg(test)]
    pub(super) const fn version(&self) -> u64 {
        self.tab_snapshot.version()
    }

    pub(super) fn line_count(&self) -> usize {
        self.fold_snapshot().line_count()
    }

    pub(super) fn slice_viewport(
        &self,
        start_row: DisplayRow,
        line_count: usize,
    ) -> DisplayMapResult<ProjectedViewportSlice<'_>> {
        self.fold_snapshot()
            .slice_viewport(ProjectedViewport::new(start_row.projected(), line_count))
    }

    pub(super) fn project_text_range(
        &self,
        range: TextRange,
    ) -> DisplayMapResult<Vec<ProjectedRange>> {
        self.fold_snapshot().project_text_range(range)
    }

    pub(super) fn offset_to_display_point(
        &self,
        offset: ByteOffset,
    ) -> DisplayMapResult<DisplayPoint> {
        offset_to_display_point(self.tab_snapshot(), offset)
    }

    pub(super) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<ByteOffset> {
        display_point_to_offset(self.tab_snapshot(), point)
    }

    pub(super) fn display_to_logical_column(
        &self,
        line: Line,
        column: DisplayColumn,
    ) -> DisplayMapResult<LogicalColumn> {
        self.tab_snapshot().display_to_logical_column(line, column)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DisplayMap {
    fold_map: FoldMap,
    tab_map: TabMap,
}

impl DisplayMap {
    pub(crate) fn new(snapshot: Snapshot) -> Self {
        let (fold_map, fold_snapshot) = FoldMap::new(snapshot);
        let (tab_map, _) = TabMap::new(fold_snapshot);
        Self { tab_map, fold_map }
    }

    pub(crate) fn buffer_snapshot(&self) -> &Snapshot {
        self.fold_map.snapshot().buffer_snapshot()
    }

    pub(super) fn snapshot(&self) -> DisplaySnapshot {
        DisplaySnapshot {
            tab_snapshot: self.tab_map.snapshot(),
        }
    }

    #[cfg(test)]
    pub(crate) fn version(&self) -> BufferVersion {
        self.fold_map.snapshot().buffer_snapshot().version()
    }

    pub(crate) fn line_count(&self) -> usize {
        self.fold_map.snapshot().line_count()
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
                    fold_map::LogicalProjection::Visible(row) => Some((row, width)),
                    fold_map::LogicalProjection::Hidden { .. } => None,
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
        self.tab_map.sync(fold_snapshot, &fold_edits);
        outcome
    }

    pub(crate) fn fold_lines(&mut self, line_range: LineRange) -> DisplayMapResult<()> {
        let (fold_snapshot, fold_edits) = self.fold_map.write().fold_lines(line_range)?;
        self.tab_map.sync(fold_snapshot, &fold_edits);
        Ok(())
    }

    pub(crate) fn offset_to_display_point(
        &self,
        offset: ByteOffset,
    ) -> DisplayMapResult<DisplayPoint> {
        offset_to_display_point(&self.tab_map.snapshot(), offset)
    }

    pub(crate) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<ByteOffset> {
        display_point_to_offset(&self.tab_map.snapshot(), point)
    }

    #[cfg(test)]
    pub(crate) fn buffer_point_to_display_point(
        &self,
        point: BufferPoint,
    ) -> DisplayMapResult<DisplayPoint> {
        self.buffer_snapshot().position_to_byte(point.position())?;
        logical_point_to_display_point(&self.tab_map.snapshot(), point.into())
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

impl From<BufferPoint> for LogicalPoint {
    fn from(point: BufferPoint) -> Self {
        Self::new(point.line(), point.column())
    }
}

fn offset_to_display_point(
    tab_snapshot: &TabSnapshot,
    offset: ByteOffset,
) -> DisplayMapResult<DisplayPoint> {
    logical_point_to_display_point(
        tab_snapshot,
        BufferPoint::from(tab_snapshot.buffer_snapshot().byte_to_position(offset)?).into(),
    )
}

fn display_point_to_offset(
    tab_snapshot: &TabSnapshot,
    point: DisplayPoint,
) -> DisplayMapResult<ByteOffset> {
    let fold_snapshot = tab_snapshot.fold_snapshot();
    let projected = ProjectedPoint::new(point.row().projected(), LogicalColumn::ZERO);
    let logical = match fold_snapshot.projected_to_logical_point(projected)? {
        ProjectedPointMapping::Text(logical) => {
            return display_column_to_byte(tab_snapshot, logical.line(), point.column());
        }
        ProjectedPointMapping::Placeholder { anchor, .. } => anchor,
    };
    Ok(fold_snapshot
        .buffer_snapshot()
        .position_to_byte(logical.into())?)
}

fn logical_point_to_display_point(
    tab_snapshot: &TabSnapshot,
    point: LogicalPoint,
) -> DisplayMapResult<DisplayPoint> {
    let fold_snapshot = tab_snapshot.fold_snapshot();
    let logical_line = point.line();
    let projected = fold_snapshot.logical_to_projected_point(point)?;
    match projected {
        LogicalPointProjection::Visible(point) => Ok(DisplayPoint::new(
            DisplayRow::from(point.line()),
            tab_snapshot.logical_to_display_column(logical_line, point.column())?,
        )),
        LogicalPointProjection::Hidden {
            anchor_projected, ..
        } => Ok(DisplayPoint::new(
            DisplayRow::from(anchor_projected.line()),
            DisplayColumn::ZERO,
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

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
        assert_eq!(viewport.len(), 3);
        assert!(viewport.rows()[1].is_placeholder());
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
}
