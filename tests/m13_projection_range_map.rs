//! M13C 机器契约：锁定 Projection 在 (line, column) point / range 层面的折叠投影语义。
//!
//! 验证范围：
//! - LogicalPoint / ProjectedPoint / LogicalRange / ProjectedRange 强类型与构造不变量;
//! - logical point <-> projected point 双向映射，可见 / 隐藏 / Text / Placeholder 四类结果;
//! - hidden 逻辑点映射到 fold anchor; placeholder 投影点映射回 fold anchor 与隐藏行区间;
//! - logical range -> projected range segments：按 row kind 切换分段，跨 fold 仍保持顺序;
//! - projected range -> logical range：placeholder 端点折叠到 fold anchor / 隐藏区结束;
//! - selection（单 / 多）通过 `project_text_range` 走 Snapshot 转换并产生投影段;
//! - snapshot 与 Projection 版本不一致时 selection 投影原子拒绝。

use zom_engine::{
    Buffer, BufferConfig, CharOffset, Edit, EngineError, FoldSet, Line, LineRange, LogicalColumn,
    LogicalPoint, LogicalPointProjection, LogicalRange, ProjectedLineIndex, ProjectedPoint,
    ProjectedPointMapping, ProjectedRange, Projection, ProjectionError, Selection, SelectionSet,
    TextRange, Transaction,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

fn line(value: usize) -> Line {
    Line::new(value)
}

fn col(value: usize) -> LogicalColumn {
    LogicalColumn::new(value)
}

fn line_range(start: usize, end: usize) -> LineRange {
    LineRange::new(line(start), line(end)).unwrap()
}

fn projected(idx: usize) -> ProjectedLineIndex {
    ProjectedLineIndex::new(idx)
}

fn lpt(line_value: usize, column_value: usize) -> LogicalPoint {
    LogicalPoint::new(line(line_value), col(column_value))
}

fn ppt(line_value: usize, column_value: usize) -> ProjectedPoint {
    ProjectedPoint::new(projected(line_value), col(column_value))
}

fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> zom_engine::DeltaEvent {
    let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
    buffer.apply_transaction(tx).unwrap();
    buffer.last_delta_event().unwrap().clone()
}

fn build_projection(buffer: &Buffer, folds: &FoldSet) -> Projection {
    Projection::build(&buffer.snapshot(), folds).unwrap()
}

fn folded_buffer() -> (Buffer, FoldSet, Projection) {
    // 文本：11 条逻辑行（编号 0..=10），fold 隐藏 logical line 4..=7（anchor 行 3）。
    let text = "L0\nL1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\nL9\nL10\n";
    let buffer = buffer(text);
    let mut folds = FoldSet::new(buffer.version());
    folds.fold_lines(&buffer, line_range(3, 8)).unwrap();
    let projection = build_projection(&buffer, &folds);
    // 投影行：proj 0=L0 ... proj 3=L3 (anchor) proj 4=Placeholder proj 5=L8 proj 6=L9 proj 7=L10 proj 8=空末行
    (buffer, folds, projection)
}

#[test]
fn logical_range_constructor_rejects_reverse_pairs() {
    let ok = LogicalRange::new(lpt(1, 0), lpt(2, 4)).unwrap();
    assert_eq!(ok.start(), lpt(1, 0));
    assert_eq!(ok.end(), lpt(2, 4));

    let same_line_reverse = LogicalRange::new(lpt(2, 5), lpt(2, 1));
    assert!(same_line_reverse.is_err());
    let line_reverse = LogicalRange::new(lpt(3, 0), lpt(1, 0));
    assert!(line_reverse.is_err());
}

#[test]
fn projected_range_constructor_rejects_reverse_pairs() {
    let ok = ProjectedRange::new(ppt(1, 0), ppt(3, 4)).unwrap();
    assert_eq!(ok.start(), ppt(1, 0));
    assert_eq!(ok.end(), ppt(3, 4));

    let reversed = ProjectedRange::new(ppt(2, 5), ppt(2, 1));
    assert!(reversed.is_err());
}

#[test]
fn visible_logical_point_maps_to_visible_projected_point_with_same_column() {
    let (_buffer, _folds, projection) = folded_buffer();
    let mapping = projection.logical_to_projected_point(lpt(1, 1)).unwrap();
    assert_eq!(mapping, LogicalPointProjection::Visible(ppt(1, 1)));

    let after_fold = projection.logical_to_projected_point(lpt(8, 0)).unwrap();
    // L8 在投影行 5（anchor 后是 placeholder + L8）
    assert_eq!(after_fold, LogicalPointProjection::Visible(ppt(5, 0)));
}

#[test]
fn hidden_logical_point_maps_to_fold_anchor_logical_and_projected_points() {
    let (_buffer, _folds, projection) = folded_buffer();
    let mapping = projection.logical_to_projected_point(lpt(5, 1)).unwrap();
    assert_eq!(
        mapping,
        LogicalPointProjection::Hidden {
            anchor_logical: lpt(3, 0),
            anchor_projected: ppt(3, 0),
        }
    );
    assert!(mapping.is_hidden());
    assert_eq!(mapping.projected_point(), ppt(3, 0));
}

#[test]
fn projected_text_point_maps_back_to_text_logical_point() {
    let (_buffer, _folds, projection) = folded_buffer();
    let mapping = projection.projected_to_logical_point(ppt(2, 1)).unwrap();
    assert_eq!(mapping, ProjectedPointMapping::Text(lpt(2, 1)));
    assert!(mapping.is_text());
    assert_eq!(mapping.logical_point(), lpt(2, 1));
}

#[test]
fn placeholder_projected_point_maps_to_fold_anchor_with_hidden_lines() {
    let (_buffer, _folds, projection) = folded_buffer();
    let mapping = projection.projected_to_logical_point(ppt(4, 0)).unwrap();
    assert_eq!(
        mapping,
        ProjectedPointMapping::Placeholder {
            anchor: lpt(3, 0),
            hidden_lines: line_range(4, 8),
        }
    );
    assert!(mapping.is_placeholder());
    assert_eq!(mapping.logical_point(), lpt(3, 0));
}

#[test]
fn logical_range_with_no_folds_in_between_produces_single_segment() {
    let buffer = buffer("a\nb\nc\nd\n");
    let folds = FoldSet::new(buffer.version());
    let projection = build_projection(&buffer, &folds);

    let segments = projection
        .logical_to_projected_range_segments(LogicalRange::new(lpt(0, 0), lpt(2, 0)).unwrap())
        .unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].start(), ppt(0, 0));
    assert_eq!(segments[0].end(), ppt(2, 0));
}

#[test]
fn empty_logical_range_produces_no_segments() {
    let (_buffer, _folds, projection) = folded_buffer();
    let segments = projection
        .logical_to_projected_range_segments(LogicalRange::new(lpt(2, 1), lpt(2, 1)).unwrap())
        .unwrap();
    assert!(segments.is_empty());
}

#[test]
fn logical_range_crossing_fold_splits_into_text_placeholder_text_segments() {
    let (_buffer, _folds, projection) = folded_buffer();
    // 选区从 L1 col 1 到 L10 col 2，跨过隐藏的 L4..=L7
    let segments = projection
        .logical_to_projected_range_segments(LogicalRange::new(lpt(1, 1), lpt(10, 2)).unwrap())
        .unwrap();
    assert_eq!(segments.len(), 3);

    // 段 1：可见文本，proj 1 col 1 -> proj 4 col 0（placeholder 起点）
    assert_eq!(segments[0].start(), ppt(1, 1));
    assert_eq!(segments[0].end(), ppt(4, 0));

    // 段 2：placeholder 单行，proj 4 -> proj 5 起点
    assert_eq!(segments[1].start(), ppt(4, 0));
    assert_eq!(segments[1].end(), ppt(5, 0));

    // 段 3：可见文本，proj 5 起点 -> proj 7 col 2（L10）
    assert_eq!(segments[2].start(), ppt(5, 0));
    assert_eq!(segments[2].end(), ppt(7, 2));
}

#[test]
fn logical_range_starting_inside_fold_collapses_start_to_anchor() {
    let (_buffer, _folds, projection) = folded_buffer();
    // 起点 L5 col 1 隐藏 -> 收缩到 anchor (proj 3, 0)
    let segments = projection
        .logical_to_projected_range_segments(LogicalRange::new(lpt(5, 1), lpt(9, 2)).unwrap())
        .unwrap();
    assert!(!segments.is_empty());
    assert_eq!(segments[0].start(), ppt(3, 0));
}

#[test]
fn logical_range_ending_inside_fold_extends_end_through_placeholder() {
    let (_buffer, _folds, projection) = folded_buffer();
    // 终点 L6 col 0 隐藏 -> 扩展到 placeholder 后第一行起点 (proj 5, 0)
    let segments = projection
        .logical_to_projected_range_segments(LogicalRange::new(lpt(1, 0), lpt(6, 0)).unwrap())
        .unwrap();
    assert!(!segments.is_empty());
    let last = segments.last().unwrap();
    assert_eq!(last.end(), ppt(5, 0));
    // 期待至少一段是 placeholder
    let has_placeholder = segments.iter().any(|seg| {
        matches!(
            projection.projected_line_kind(seg.start().line()),
            Some(zom_engine::ProjectedLineKind::Placeholder(_))
        )
    });
    assert!(has_placeholder);
}

#[test]
fn projected_range_back_to_logical_collapses_placeholder_endpoints_to_anchor_or_hidden_end() {
    let (_buffer, _folds, projection) = folded_buffer();
    // 投影范围：从 placeholder 起点到 L9 col 2
    let projected_range = ProjectedRange::new(ppt(4, 0), ppt(6, 2)).unwrap();
    let logical_range = projection
        .projected_to_logical_range(projected_range)
        .unwrap();
    // Placeholder 起点 -> fold anchor 行起点 (L3, 0)
    assert_eq!(logical_range.start(), lpt(3, 0));
    // 终点为 TextLine -> (L9, 2)
    assert_eq!(logical_range.end(), lpt(9, 2));
}

#[test]
fn projected_range_with_placeholder_end_maps_to_hidden_lines_end() {
    let (_buffer, _folds, projection) = folded_buffer();
    // 投影范围：从 L1 col 1 一直到 placeholder 行内某点
    let projected_range = ProjectedRange::new(ppt(1, 1), ppt(4, 0)).unwrap();
    let logical_range = projection
        .projected_to_logical_range(projected_range)
        .unwrap();
    assert_eq!(logical_range.start(), lpt(1, 1));
    // Placeholder 终点 -> hidden_lines.end = L8
    assert_eq!(logical_range.end(), lpt(8, 0));
}

#[test]
fn project_text_range_takes_selection_range_through_snapshot_into_segments() {
    let (buffer, _folds, projection) = folded_buffer();
    // Selection 覆盖从 L1 col 1 到 L10 col 2 的 char 范围
    let start = c(buffer.line_start(line(1)).unwrap().get() + 1);
    let end = c(buffer.line_start(line(10)).unwrap().get() + 2);
    let selection = Selection::new(start, end);
    let segments = projection
        .project_text_range(&buffer.snapshot(), selection.range())
        .unwrap();

    assert_eq!(segments.len(), 3);
    assert_eq!(segments[0].start(), ppt(1, 1));
    assert_eq!(segments[1].start().line(), projected(4));
    assert_eq!(segments[2].end(), ppt(7, 2));
}

#[test]
fn multi_selection_through_fold_can_be_projected_per_selection() {
    let (buffer, _folds, projection) = folded_buffer();
    let s1 = Selection::new(c(0), c(buffer.line_start(line(1)).unwrap().get() + 2));
    let s2 = Selection::new(
        buffer.line_start(line(5)).unwrap(),
        c(buffer.line_start(line(9)).unwrap().get() + 1),
    );
    let selections = SelectionSet::new(vec![s1, s2]);
    let snapshot = buffer.snapshot();
    let projected_per_selection: Vec<Vec<ProjectedRange>> = selections
        .as_slice()
        .iter()
        .map(|s| projection.project_text_range(&snapshot, s.range()).unwrap())
        .collect();

    assert_eq!(projected_per_selection.len(), 2);
    // 第一段不跨 fold，单段
    assert_eq!(projected_per_selection[0].len(), 1);
    assert_eq!(projected_per_selection[0][0].start(), ppt(0, 0));
    // 第二段起点在 fold 内 -> 收缩到 anchor
    assert_eq!(projected_per_selection[1][0].start(), ppt(3, 0));
}

#[test]
fn project_text_range_rejects_snapshot_with_different_version() {
    let (mut buffer, folds, projection) = folded_buffer();
    apply(
        &mut buffer,
        vec![Edit::insert(c(0), "X".to_string()).unwrap()],
    );
    let new_snapshot = buffer.snapshot();
    let err = projection
        .project_text_range(&new_snapshot, TextRange::new(c(0), c(2)).unwrap())
        .unwrap_err();
    assert!(matches!(
        err,
        EngineError::Projection(ProjectionError::VersionMismatch { .. })
    ));
    let _ = folds; // suppress unused
}

#[test]
fn out_of_bounds_logical_point_returns_coordinate_error() {
    let (_buffer, _folds, projection) = folded_buffer();
    let err = projection
        .logical_to_projected_point(lpt(99, 0))
        .unwrap_err();
    assert!(matches!(err, EngineError::Coordinate(_)));
}
