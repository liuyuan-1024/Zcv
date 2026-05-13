use zom_engine::*;

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn line(value: usize) -> Line {
    Line::new(value)
}

fn col(value: usize) -> LogicalColumn {
    LogicalColumn::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

fn line_range(start: usize, end: usize) -> LineRange {
    LineRange::new(line(start), line(end)).unwrap()
}

fn projected(value: usize) -> ProjectedLineIndex {
    ProjectedLineIndex::new(value)
}

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

#[test]
fn fold_set_should_reject_empty_and_partial_overlap_while_allowing_exact_toggle() {
    let mut folds = FoldSet::new(BufferVersion::INITIAL);
    let id = folds.fold(range(1, 4)).unwrap();

    assert_eq!(folds.fold(range(1, 4)).unwrap(), id);
    assert!(matches!(
        folds.fold(range(2, 5)).unwrap_err(),
        FoldError::OverlapWithoutNesting { .. }
    ));
    assert!(matches!(
        folds.fold(range(3, 3)).unwrap_err(),
        FoldError::EmptyRange { .. }
    ));
    assert!(matches!(
        folds.toggle(range(1, 4)).unwrap(),
        FoldToggleOutcome::Unfolded(removed) if removed == id
    ));
    assert!(folds.is_empty());
}

#[test]
fn fold_lines_hidden_ranges_should_hide_lines_after_anchor_until_fold_end() {
    let buffer = buffer("a\nb\nc\nd");
    let mut folds = FoldSet::new(buffer.version());
    let id = folds.fold_lines(&buffer, line_range(1, 3)).unwrap();
    let hidden = folds.derive_hidden_ranges(&buffer).unwrap();

    assert_eq!(folds.get(id).unwrap().range(), range(2, 6));
    assert!(folds.is_line_hidden(&buffer, line(2)).unwrap());
    assert!(!folds.is_line_hidden(&buffer, line(1)).unwrap());
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0].first_hidden_line(), line(2));
    assert_eq!(hidden[0].end_line_exclusive(), line(3));
    assert!(hidden[0].contains_line(line(2)));
}

#[test]
fn fold_set_update_through_delta_should_advance_version_or_reject_mismatch_atomically() {
    let mut buffer = buffer("abcdef");
    let mut folds = FoldSet::new(buffer.version());
    folds.fold(range(2, 5)).unwrap();

    buffer.insert(b(0), "X").unwrap();
    let event = buffer.last_delta_event().unwrap().clone();
    let updates = folds.update_through_delta_event(&event).unwrap();

    assert_eq!(updates.len(), 1);
    assert_eq!(folds.version(), event.new_version());
    assert_eq!(folds.as_slice()[0].range(), range(3, 6));

    let stale = folds.update_through_delta_event(&event).unwrap_err();
    assert!(matches!(stale, FoldError::VersionMismatch { .. }));
}

#[test]
fn projection_build_should_reject_snapshot_and_fold_version_mismatch() {
    let mut buffer = buffer("a\nb");
    let snapshot = buffer.snapshot();
    buffer.insert(b(0), "x").unwrap();
    let folds = FoldSet::new(buffer.version());

    let err = Projection::build(&snapshot, &folds).unwrap_err();

    assert!(matches!(
        err,
        EngineError::Projection(ProjectionError::VersionMismatch { .. })
    ));
}

#[test]
fn projection_line_map_should_distinguish_text_rows_hidden_rows_and_placeholder_rows() {
    let buffer = buffer("a\nb\nc\nd");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    folds.fold_lines(&buffer, line_range(1, 3)).unwrap();
    let projection = Projection::build(&snapshot, &folds).unwrap();

    assert_eq!(projection.logical_line_count(), 4);
    assert_eq!(projection.line_count(), 4);
    assert!(
        projection
            .projected_line_kind(projected(2))
            .unwrap()
            .is_placeholder()
    );
    assert!(
        projection
            .logical_to_projected(line(2))
            .unwrap()
            .is_hidden()
    );
    assert_eq!(
        projection.fold_anchor_for_logical_line(line(2)).unwrap(),
        line(1)
    );
    assert_eq!(
        projection.fold_anchor_for_projected_line(projected(2)),
        Some(line(1))
    );
    assert!(projection.is_logical_line_hidden(line(2)).unwrap());
}

#[test]
fn projection_point_and_range_mapping_should_return_typed_hidden_and_placeholder_facts() {
    let buffer = buffer("a\nb\nc\nd");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    folds.fold_lines(&buffer, line_range(1, 3)).unwrap();
    let projection = Projection::build(&snapshot, &folds).unwrap();

    let hidden = projection
        .logical_to_projected_point(LogicalPoint::new(line(2), col(1)))
        .unwrap();
    let placeholder = projection
        .projected_to_logical_point(ProjectedPoint::line_start(projected(2)))
        .unwrap();
    let segments = projection
        .logical_to_projected_range_segments(
            LogicalRange::new(
                LogicalPoint::line_start(line(0)),
                LogicalPoint::line_start(line(3)),
            )
            .unwrap(),
        )
        .unwrap();

    assert!(hidden.is_hidden());
    assert_eq!(
        hidden.projected_point(),
        ProjectedPoint::line_start(projected(1))
    );
    assert!(placeholder.is_placeholder());
    assert_eq!(
        placeholder.logical_point(),
        LogicalPoint::line_start(line(1))
    );
    assert!(!segments.is_empty());
}

#[test]
fn projected_viewport_should_emit_text_and_placeholder_rows_with_logical_spans() {
    let buffer = buffer("alpha\nbravo\ncharlie\ndelta");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    folds.fold_lines(&buffer, line_range(1, 3)).unwrap();
    let projection = Projection::build(&snapshot, &folds).unwrap();

    let slice = projection
        .slice_viewport(
            &snapshot,
            ProjectedViewport::new(projected(1), 3).with_max_line_chars(2),
        )
        .unwrap();

    assert_eq!(slice.projected_line_range().start(), projected(1));
    assert_eq!(slice.len(), 3);
    assert!(slice.rows()[0].is_text());
    assert!(slice.rows()[1].is_placeholder());
    assert_eq!(slice.rows()[0].kind().logical_line(), Some(line(1)));
    assert_eq!(
        slice.rows()[0].kind().visible_line().unwrap().as_str(),
        "br"
    );
    assert_eq!(slice.placeholders().len(), 1);
    assert_eq!(
        slice.logical_line_spans(),
        &[line_range(1, 2), line_range(3, 4)]
    );
}
