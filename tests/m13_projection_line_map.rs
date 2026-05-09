//! M13B 机器契约：锁定 Projection / ProjectedLine / ProjectedLineKind / TextLine /
//! FoldPlaceholder 的行级折叠投影语义。
//!
//! 验证范围：
//! - 从 `Snapshot + FoldSet` 构建 Projection，版本不匹配原子拒绝;
//! - logical line -> projected line 的 Visible / Hidden 双向映射;
//! - hidden 逻辑行 -> fold anchor 行 的回溯;
//! - fold placeholder 投影行 -> fold anchor 的回溯;
//! - 嵌套与邻接 fold 在投影空间合并为单条占位符;
//! - intra-line fold 不在 M13B 产生 placeholder 行;
//! - projection 总行数 = 可见逻辑行数 + 占位符数;
//! - projection 一旦构建即不可变（不会因后续编辑被修改）。

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, Edit, EngineError, FoldSet, Line, LineRange,
    LogicalProjection, ProjectedLineIndex, ProjectedLineKind, Projection, ProjectionError,
    TextRange, Transaction,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(c(start), c(end)).unwrap()
}

fn line(value: usize) -> Line {
    Line::new(value)
}

fn line_range(start: usize, end: usize) -> LineRange {
    LineRange::new(line(start), line(end)).unwrap()
}

fn projected(idx: usize) -> ProjectedLineIndex {
    ProjectedLineIndex::new(idx)
}

fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> zom_engine::DeltaEvent {
    let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
    buffer.apply_transaction(tx).unwrap();
    buffer.last_delta_event().unwrap().clone()
}

#[test]
fn projection_with_no_folds_maps_logical_to_projected_one_to_one() {
    let buffer = buffer("a\nb\nc\nd\n");
    let folds = FoldSet::new(buffer.version());
    let projection = Projection::build(&buffer.snapshot(), &folds).unwrap();

    assert_eq!(projection.line_count(), 5); // 4 visible lines + trailing empty line
    assert_eq!(projection.logical_line_count(), 5);
    for line_value in 0..5 {
        let mapping = projection.logical_to_projected(line(line_value)).unwrap();
        assert_eq!(mapping, LogicalProjection::Visible(projected(line_value)));
    }
    for index in 0..5 {
        let projected_line = projection.projected_line(projected(index)).unwrap();
        let text_line = projected_line.kind().text_line().unwrap();
        assert_eq!(text_line.logical_line(), line(index));
    }
}

#[test]
fn projection_inserts_single_placeholder_for_multi_line_fold() {
    let buffer = buffer("a\nb\nc\nd\ne\n");
    let mut folds = FoldSet::new(buffer.version());
    folds.fold_lines(&buffer, line_range(0, 3)).unwrap();
    let projection = Projection::build(&buffer.snapshot(), &folds).unwrap();

    // anchor=0, hidden lines=1..3, placeholder, line 3, line 4, trailing line 5
    // logical line count = 6 (5 \n's), projected = 1 anchor + 1 placeholder + 3 visible = 5
    assert_eq!(projection.logical_line_count(), 6);
    assert_eq!(projection.line_count(), 5);

    assert_eq!(
        projection.logical_to_projected(line(0)).unwrap(),
        LogicalProjection::Visible(projected(0))
    );
    let hidden_one = projection.logical_to_projected(line(1)).unwrap();
    assert_eq!(
        hidden_one,
        LogicalProjection::Hidden {
            anchor_logical_line: line(0),
            anchor_projected_line: projected(0),
        }
    );
    assert!(hidden_one.is_hidden());
    assert!(projection.is_logical_line_hidden(line(2)).unwrap());
    assert!(!projection.is_logical_line_hidden(line(3)).unwrap());

    let placeholder_kind = projection.projected_line_kind(projected(1)).unwrap();
    let placeholder = placeholder_kind.placeholder().unwrap();
    assert_eq!(placeholder.anchor_line(), line(0));
    assert_eq!(placeholder.hidden_lines(), line_range(1, 3));
    assert_eq!(placeholder.hidden_line_count(), 2);

    // line 3 visible at projected index 2 (after anchor + placeholder)
    let mapping = projection.logical_to_projected(line(3)).unwrap();
    assert_eq!(mapping, LogicalProjection::Visible(projected(2)));
}

#[test]
fn projected_line_iter_has_index_aligned_with_position() {
    let buffer = buffer("a\nb\nc\nd\n");
    let mut folds = FoldSet::new(buffer.version());
    folds.fold_lines(&buffer, line_range(1, 3)).unwrap();
    let projection = Projection::build(&buffer.snapshot(), &folds).unwrap();

    let lines: Vec<_> = projection.iter().collect();
    assert_eq!(lines.len(), projection.line_count());
    for (i, projected_line) in lines.iter().enumerate() {
        assert_eq!(projected_line.index(), projected(i));
    }
}

#[test]
fn fold_anchor_lookup_returns_anchor_for_hidden_and_self_for_visible() {
    let buffer = buffer("a\nb\nc\nd\n");
    let mut folds = FoldSet::new(buffer.version());
    folds.fold_lines(&buffer, line_range(0, 3)).unwrap();
    let projection = Projection::build(&buffer.snapshot(), &folds).unwrap();

    assert_eq!(
        projection.fold_anchor_for_logical_line(line(0)).unwrap(),
        line(0)
    );
    assert_eq!(
        projection.fold_anchor_for_logical_line(line(1)).unwrap(),
        line(0)
    );
    assert_eq!(
        projection.fold_anchor_for_logical_line(line(2)).unwrap(),
        line(0)
    );
    assert_eq!(
        projection.fold_anchor_for_logical_line(line(3)).unwrap(),
        line(3)
    );
}

#[test]
fn placeholder_projected_line_maps_back_to_fold_anchor() {
    let buffer = buffer("a\nb\nc\nd\n");
    let mut folds = FoldSet::new(buffer.version());
    folds.fold_lines(&buffer, line_range(0, 3)).unwrap();
    let projection = Projection::build(&buffer.snapshot(), &folds).unwrap();

    let placeholder_index = projected(1);
    let kind = projection.projected_line_kind(placeholder_index).unwrap();
    assert!(kind.is_placeholder());
    assert_eq!(
        projection.fold_anchor_for_projected_line(placeholder_index),
        Some(line(0))
    );
    assert_eq!(
        projection.fold_anchor_for_projected_line(projected(0)),
        None
    );
    assert_eq!(
        projection.fold_anchor_for_projected_line(projected(2)),
        None
    );
}

#[test]
fn nested_folds_share_one_placeholder() {
    let buffer = buffer("a\nb\nc\nd\ne\nf\n");
    let mut folds = FoldSet::new(buffer.version());
    folds.fold_lines(&buffer, line_range(0, 4)).unwrap();
    folds.fold_lines(&buffer, line_range(1, 3)).unwrap();
    let projection = Projection::build(&buffer.snapshot(), &folds).unwrap();

    let placeholder_count = projection
        .iter()
        .filter(|line| line.kind().is_placeholder())
        .count();
    assert_eq!(placeholder_count, 1);

    let placeholder = projection
        .iter()
        .find(|line| line.kind().is_placeholder())
        .unwrap();
    let placeholder = placeholder.kind().placeholder().unwrap();
    assert_eq!(placeholder.anchor_line(), line(0));
    assert_eq!(placeholder.hidden_lines(), line_range(1, 4));
}

#[test]
fn non_nested_folds_each_produce_their_own_placeholder() {
    // 两个非嵌套 fold 之间至少存在一条可见 anchor 行（line 2 与 line 5），
    // 因此投影空间里它们不会被合并到同一条 placeholder。
    let buffer = buffer("a\nb\nc\nd\ne\nf\n");
    let mut folds = FoldSet::new(buffer.version());
    folds.fold_lines(&buffer, line_range(0, 2)).unwrap();
    folds.fold_lines(&buffer, line_range(2, 5)).unwrap();
    let projection = Projection::build(&buffer.snapshot(), &folds).unwrap();

    let placeholders: Vec<_> = projection
        .iter()
        .filter_map(|line| line.kind().placeholder())
        .collect();
    assert_eq!(placeholders.len(), 2);
    assert_eq!(placeholders[0].anchor_line(), line(0));
    assert_eq!(placeholders[0].hidden_lines(), line_range(1, 2));
    assert_eq!(placeholders[1].anchor_line(), line(2));
    assert_eq!(placeholders[1].hidden_lines(), line_range(3, 5));
}

#[test]
fn intra_line_folds_do_not_produce_placeholder_lines() {
    let buffer = buffer("hello world\n");
    let mut folds = FoldSet::new(buffer.version());
    folds.fold(range(2, 5)).unwrap();
    let projection = Projection::build(&buffer.snapshot(), &folds).unwrap();

    assert_eq!(projection.line_count(), 2);
    for line_value in 0..2 {
        let kind = projection
            .projected_line_kind(projected(line_value))
            .unwrap();
        assert!(kind.is_text());
    }
}

#[test]
fn build_rejects_version_mismatch_between_snapshot_and_folds() {
    let mut buffer = buffer("a\nb\n");
    let folds = FoldSet::new(buffer.version());
    apply(
        &mut buffer,
        vec![Edit::insert(c(0), "X".to_string()).unwrap()],
    );
    let stale_snapshot = buffer.snapshot();
    let err = Projection::build(&stale_snapshot, &folds).unwrap_err();

    let expected_snapshot_version = buffer.version();
    assert_eq!(
        err,
        EngineError::Projection(ProjectionError::VersionMismatch {
            snapshot_version: expected_snapshot_version,
            fold_version: BufferVersion::INITIAL,
        })
    );
}

#[test]
fn projection_is_immutable_after_build_even_when_buffer_advances() {
    let mut buffer = buffer("a\nb\nc\nd\n");
    let mut folds = FoldSet::new(buffer.version());
    folds.fold_lines(&buffer, line_range(0, 2)).unwrap();
    let original = Projection::build(&buffer.snapshot(), &folds).unwrap();
    let original_version = original.version();
    let original_line_count = original.line_count();

    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(0), "XYZ".to_string()).unwrap()],
    );
    folds.update_through_delta_event(&event).unwrap();

    assert_eq!(original.version(), original_version);
    assert_eq!(original.line_count(), original_line_count);
    assert!(original.is_stale_for_version(buffer.version()));
}

#[test]
fn out_of_bounds_logical_line_query_returns_coordinate_error() {
    let buffer = buffer("a\nb\n");
    let folds = FoldSet::new(buffer.version());
    let projection = Projection::build(&buffer.snapshot(), &folds).unwrap();
    let err = projection.logical_to_projected(line(99)).unwrap_err();
    assert!(matches!(err, EngineError::Coordinate(_)));
}

#[test]
fn projected_line_count_matches_visible_logical_lines_plus_placeholders() {
    let buffer = buffer("a\nb\nc\nd\ne\nf\ng\nh\n");
    let mut folds = FoldSet::new(buffer.version());
    folds.fold_lines(&buffer, line_range(0, 3)).unwrap();
    folds.fold_lines(&buffer, line_range(5, 7)).unwrap();
    let projection = Projection::build(&buffer.snapshot(), &folds).unwrap();

    let visible_logical = (0..projection.logical_line_count())
        .filter(|&i| {
            !matches!(
                projection.logical_to_projected(line(i)).unwrap(),
                LogicalProjection::Hidden { .. }
            )
        })
        .count();
    let placeholders = projection
        .iter()
        .filter(|line| line.kind().is_placeholder())
        .count();

    assert_eq!(projection.line_count(), visible_logical + placeholders);
    assert_eq!(placeholders, 2);
}

#[test]
fn projected_line_kind_text_carries_logical_line_index() {
    let buffer = buffer("a\nb\nc\nd\n");
    let folds = FoldSet::new(buffer.version());
    let projection = Projection::build(&buffer.snapshot(), &folds).unwrap();

    let kind = projection.projected_line_kind(projected(2)).unwrap();
    match kind {
        ProjectedLineKind::Text(text_line) => {
            assert_eq!(text_line.logical_line(), line(2));
        }
        ProjectedLineKind::Placeholder(_) => panic!("expected text line"),
    }
}

#[test]
fn projection_error_propagates_through_engine_error_umbrella() {
    let buffer = buffer("a\n");
    let folds = FoldSet::new(BufferVersion::new(7));
    let err: EngineError = Projection::build(&buffer.snapshot(), &folds).unwrap_err();
    assert!(matches!(
        err,
        EngineError::Projection(ProjectionError::VersionMismatch { .. })
    ));
}
