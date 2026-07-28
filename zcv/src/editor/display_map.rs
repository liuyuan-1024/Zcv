//! Buffer 逻辑坐标到 Editor 显示坐标的投影。

mod error;
mod fold;
mod projection;
mod tab_map;

#[cfg(test)]
mod test;

use std::sync::Arc;

use zcv_engine::{
    ByteOffset, DeltaEvent, DisplayColumn, Line, LogicalColumn, Position, Snapshot, TextRange,
};

#[cfg(test)]
use zcv_engine::{BufferVersion, LineRange};

use error::DisplayMapResult;
use fold::FoldSet;
use projection::{
    ApplyOutcome, LogicalPoint, LogicalPointProjection, ProjectedPoint, ProjectedPointMapping,
    ProjectedViewport, ProjectedViewportSlice, Projection,
};
pub(crate) use projection::{ProjectedLineIndex, ProjectedRange, ProjectedViewportRowKind};
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
/// `Snapshot` 与 `Projection` 都是低成本克隆；渲染持有此值时不会阻塞 Editor
/// 接收后续 Buffer 更新。
#[derive(Debug, Clone)]
pub(super) struct DisplaySnapshot {
    snapshot: Snapshot,
    projection: Arc<Projection>,
    tab_snapshot: TabSnapshot,
}

impl DisplaySnapshot {
    pub(super) fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub(super) fn line_count(&self) -> usize {
        self.projection.line_count()
    }

    pub(super) fn slice_viewport(
        &self,
        start_row: DisplayRow,
        line_count: usize,
    ) -> DisplayMapResult<ProjectedViewportSlice<'_>> {
        self.projection.slice_viewport(
            &self.snapshot,
            ProjectedViewport::new(start_row.projected(), line_count),
        )
    }

    pub(super) fn project_text_range(
        &self,
        range: TextRange,
    ) -> DisplayMapResult<Vec<ProjectedRange>> {
        self.projection.project_text_range(&self.snapshot, range)
    }

    pub(super) fn offset_to_display_point(
        &self,
        offset: ByteOffset,
    ) -> DisplayMapResult<DisplayPoint> {
        offset_to_display_point(&self.snapshot, &self.projection, offset)
    }

    pub(super) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<ByteOffset> {
        display_point_to_offset(&self.snapshot, &self.projection, &self.tab_snapshot, point)
    }

    pub(super) fn display_to_logical_column(
        &self,
        line: Line,
        column: DisplayColumn,
    ) -> DisplayMapResult<LogicalColumn> {
        self.tab_snapshot.display_to_logical_column(line, column)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DisplayMap {
    snapshot: Snapshot,
    folds: FoldSet,
    projection: Arc<Projection>,
    tab_map: TabMap,
}

impl DisplayMap {
    pub(crate) fn new(snapshot: Snapshot) -> Self {
        let folds = FoldSet::new(snapshot.version());
        let projection = Projection::build(&snapshot, &folds)
            .expect("空 FoldSet 与同版本 Snapshot 必须能建立 Projection");
        Self {
            tab_map: TabMap::new(snapshot.clone()),
            snapshot,
            folds,
            projection: Arc::new(projection),
        }
    }

    pub(crate) fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub(super) fn display_snapshot(&self) -> DisplaySnapshot {
        DisplaySnapshot {
            snapshot: self.snapshot.clone(),
            projection: Arc::clone(&self.projection),
            tab_snapshot: self.tab_map.snapshot(),
        }
    }

    #[cfg(test)]
    pub(crate) fn version(&self) -> BufferVersion {
        self.snapshot.version()
    }

    pub(crate) fn line_count(&self) -> usize {
        self.projection.line_count()
    }

    pub(crate) fn measure_rows(
        &mut self,
        start_row: DisplayRow,
        line_count: usize,
    ) -> DisplayMapResult<()> {
        let end = start_row
            .get()
            .saturating_add(line_count)
            .min(self.projection.line_count());
        for row in start_row.get()..end {
            if let Some(projected) = self.projection.projected_line(ProjectedLineIndex::new(row))
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
            .filter_map(
                |(line, width)| match self.projection.logical_to_projected(line).ok()? {
                    projection::LogicalProjection::Visible(row) => Some((row, width)),
                    projection::LogicalProjection::Hidden { .. } => None,
                },
            )
            .max_by_key(|(_, width)| *width)
            .map(|(row, _)| DisplayRow::from(row))
            .unwrap_or(DisplayRow::ZERO)
    }

    /// 把 DisplayMap 推进到新的 Buffer Snapshot。
    ///
    /// 正常编辑路径通过同版本链的 `DeltaEvent` 增量更新。若宿主漏过事件，无法安全
    /// 推进已有 fold 的 tracked range，只能清空 fold 并按新快照防御性重建。
    pub(crate) fn sync_snapshot(
        &mut self,
        snapshot: Snapshot,
        event: Option<&DeltaEvent>,
    ) -> ApplyOutcome {
        if snapshot.version() == self.snapshot.version() {
            return ApplyOutcome::Compatible;
        }

        let can_apply = event.is_some_and(|event| {
            event.old_version() == self.snapshot.version()
                && event.new_version() == snapshot.version()
        });
        if !can_apply {
            self.reset_for_snapshot(snapshot);
            return ApplyOutcome::Rebuilt;
        }

        let event = event.expect("can_apply 已确认 DeltaEvent 存在");
        if self
            .folds
            .update_through_delta_event(event, &snapshot)
            .is_err()
        {
            self.reset_for_snapshot(snapshot);
            return ApplyOutcome::Rebuilt;
        }

        let outcome = Arc::make_mut(&mut self.projection)
            .apply_delta(&snapshot, &self.folds, event)
            .unwrap_or(ApplyOutcome::Rebuilt);
        self.snapshot = snapshot;
        self.tab_map.sync(self.snapshot.clone(), Some(event));

        if self.projection.version() != self.snapshot.version() {
            self.projection = Arc::new(
                Projection::build(&self.snapshot, &self.folds)
                    .expect("同版本 FoldSet 与 Snapshot 必须能重建 Projection"),
            );
        }
        outcome
    }

    #[cfg(test)]
    pub(crate) fn fold_lines(&mut self, line_range: LineRange) -> DisplayMapResult<()> {
        self.folds.fold_lines(&self.snapshot, line_range)?;
        self.rebuild_projection();
        Ok(())
    }

    pub(crate) fn offset_to_display_point(
        &self,
        offset: ByteOffset,
    ) -> DisplayMapResult<DisplayPoint> {
        offset_to_display_point(&self.snapshot, &self.projection, offset)
    }

    pub(crate) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<ByteOffset> {
        display_point_to_offset(
            &self.snapshot,
            &self.projection,
            &self.tab_map.snapshot(),
            point,
        )
    }

    #[cfg(test)]
    pub(crate) fn buffer_point_to_display_point(
        &self,
        point: BufferPoint,
    ) -> DisplayMapResult<DisplayPoint> {
        self.snapshot.position_to_byte(point.position())?;
        logical_point_to_display_point(&self.snapshot, &self.projection, point.into())
    }

    #[cfg(test)]
    pub(crate) fn display_point_to_buffer_point(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<BufferPoint> {
        let offset = self.display_point_to_offset(point)?;
        Ok(self
            .snapshot
            .byte_to_position(offset)
            .map(BufferPoint::from)?)
    }

    fn reset_for_snapshot(&mut self, snapshot: Snapshot) {
        self.folds = FoldSet::new(snapshot.version());
        self.snapshot = snapshot;
        self.tab_map.sync(self.snapshot.clone(), None);
        self.projection = Arc::new(
            Projection::build(&self.snapshot, &self.folds)
                .expect("空 FoldSet 与同版本 Snapshot 必须能重建 Projection"),
        );
    }

    #[cfg(test)]
    fn rebuild_projection(&mut self) {
        self.projection = Arc::new(
            Projection::build(&self.snapshot, &self.folds)
                .expect("同版本 FoldSet 与 Snapshot 必须能重建 Projection"),
        );
    }
}

impl From<BufferPoint> for LogicalPoint {
    fn from(point: BufferPoint) -> Self {
        Self::new(point.line(), point.column())
    }
}

fn offset_to_display_point(
    snapshot: &Snapshot,
    projection: &Projection,
    offset: ByteOffset,
) -> DisplayMapResult<DisplayPoint> {
    logical_point_to_display_point(
        snapshot,
        projection,
        BufferPoint::from(snapshot.byte_to_position(offset)?).into(),
    )
}

fn display_point_to_offset(
    snapshot: &Snapshot,
    projection: &Projection,
    tab_snapshot: &TabSnapshot,
    point: DisplayPoint,
) -> DisplayMapResult<ByteOffset> {
    let projected = ProjectedPoint::new(point.row().projected(), LogicalColumn::ZERO);
    let logical = match projection.projected_to_logical_point(projected)? {
        ProjectedPointMapping::Text(logical) => {
            return display_column_to_byte(snapshot, tab_snapshot, logical.line(), point.column());
        }
        ProjectedPointMapping::Placeholder { anchor, .. } => anchor,
    };
    Ok(snapshot.position_to_byte(logical.into())?)
}

fn logical_point_to_display_point(
    snapshot: &Snapshot,
    projection: &Projection,
    point: LogicalPoint,
) -> DisplayMapResult<DisplayPoint> {
    let logical_line = point.line();
    let projected = projection.logical_to_projected_point(point)?;
    match projected {
        LogicalPointProjection::Visible(point) => Ok(DisplayPoint::new(
            DisplayRow::from(point.line()),
            TabSnapshot::new(snapshot.clone())
                .logical_to_display_column(logical_line, point.column())?,
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
        assert_eq!(map.snapshot().len_bytes(), ByteOffset::new(1));
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
        map.fold_lines(LineRange::new(Line::ZERO, Line::new(3)).expect("测试行区间应合法"))
            .expect("折叠应成功");

        assert_eq!(map.line_count(), 3);
        assert_eq!(
            map.offset_to_display_point(ByteOffset::new("anchor\nhidden ".len()))
                .expect("隐藏位置应能投影")
                .row(),
            DisplayRow::ZERO
        );

        let snapshot = map.display_snapshot();
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
        buffer
            .insert(ByteOffset::new(5), " becomes longest")
            .expect("测试编辑应成功");
        let event = buffer
            .last_delta_event()
            .expect("成功事务应产生 DeltaEvent")
            .clone();
        let outcome = map.sync_snapshot(buffer.snapshot(), Some(&event));

        assert_eq!(outcome, ApplyOutcome::Compatible);
        assert_eq!(map.longest_measured_row(), DisplayRow::new(1));
        map.measure_rows(DisplayRow::ZERO, 1)
            .expect("变更行应能按需重新测量");
        assert_eq!(map.longest_measured_row(), DisplayRow::ZERO);
    }

    #[test]
    fn structural_edit_clears_tab_measurements_until_rows_are_requested_again() {
        let mut buffer = Buffer::scratch("short\nwide".to_string(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.measure_rows(DisplayRow::ZERO, 2)
            .expect("测试显示行应能测量");
        buffer
            .insert(ByteOffset::new(5), "\nvery very wide")
            .expect("测试编辑应成功");
        let event = buffer
            .last_delta_event()
            .expect("成功事务应产生 DeltaEvent")
            .clone();

        assert_eq!(
            map.sync_snapshot(buffer.snapshot(), Some(&event)),
            ApplyOutcome::Spliced
        );
        assert_eq!(map.longest_measured_row(), DisplayRow::ZERO);
        map.measure_rows(DisplayRow::new(1), 1)
            .expect("结构编辑后的行应能惰性测量");
        assert_eq!(map.longest_measured_row(), DisplayRow::new(1));
    }
}
