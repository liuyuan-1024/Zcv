use super::super::{
    error::{DisplayMapError, FoldError, ProjectionError},
    fold::*,
    projection::*,
};
use super::test_helpers::*;

#[test]
fn fold_set_should_reject_empty_and_partial_overlap_while_allowing_exact_toggle() {
    let buffer = buffer("abcdef");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(buffer.version());
    let id = folds.fold(&snapshot, range(1, 4)).unwrap();

    assert_eq!(folds.fold(&snapshot, range(1, 4)).unwrap(), id);
    let overlap = folds.fold(&snapshot, range(2, 5)).unwrap_err();
    assert!(matches!(
        overlap,
        DisplayMapError::Fold(FoldError::OverlapWithoutNesting { .. })
    ));
    let empty = folds.fold(&snapshot, range(3, 3)).unwrap_err();
    assert!(matches!(
        empty,
        DisplayMapError::Fold(FoldError::EmptyRange { .. })
    ));
    assert!(matches!(
        folds.toggle(&snapshot, range(1, 4)).unwrap(),
        FoldToggleOutcome::Unfolded(removed) if removed == id
    ));
    assert!(folds.is_empty());
}

#[test]
fn fold_lines_hidden_ranges_should_hide_lines_after_anchor_until_fold_end() {
    let buffer = buffer("a\nb\nc\nd");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(buffer.version());
    let id = folds.fold_lines(&snapshot, line_range(1, 3)).unwrap();
    let hidden = folds.derive_hidden_ranges().unwrap();

    assert_eq!(folds.get(id).unwrap().range(), range(2, 6));
    assert!(folds.is_line_hidden(line(2)));
    assert!(!folds.is_line_hidden(line(1)));
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0].first_hidden_line(), line(2));
    assert_eq!(hidden[0].end_line_exclusive(), line(3));
    assert!(hidden[0].contains_line(line(2)));
}

#[test]
fn fold_set_update_through_patch_should_advance_version_or_reject_mismatch_atomically() {
    let mut buffer = buffer("abcdef");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(buffer.version());
    folds.fold(&snapshot, range(2, 5)).unwrap();

    let subscription = buffer.subscribe();
    let old_version = buffer.version();
    buffer.insert(b(0), "X").unwrap();
    let changes = subscription.consume();
    let new_snapshot = buffer.snapshot();
    let updates = folds
        .update_through_patch(
            old_version,
            buffer.version(),
            changes.patch(),
            &new_snapshot,
        )
        .unwrap();

    assert_eq!(updates.len(), 1);
    assert_eq!(folds.version(), buffer.version());
    assert_eq!(folds.iter().next().unwrap().range(), range(3, 6));

    let stale = folds
        .update_through_patch(
            old_version,
            buffer.version(),
            changes.patch(),
            &new_snapshot,
        )
        .unwrap_err();
    assert!(matches!(
        stale,
        DisplayMapError::Fold(FoldError::VersionMismatch { .. })
    ));
}

#[test]
fn fold_set_sum_tree_should_keep_sorted_order_id_lookup_and_persistent_clones() {
    let buffer = buffer("zero\none\ntwo\nthree\nfour\nfive\n");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());

    let late = folds.fold_lines(&snapshot, line_range(4, 6)).unwrap();
    let early = folds.fold_lines(&snapshot, line_range(0, 2)).unwrap();
    let middle = folds.fold_lines(&snapshot, line_range(2, 4)).unwrap();
    let preserved = folds.clone();

    let starts: Vec<_> = folds.iter().map(FoldRange::start_line).collect();
    assert_eq!(starts, vec![line(0), line(2), line(4)]);
    assert_eq!(folds.get(early).unwrap().start_line(), line(0));
    assert_eq!(folds.get(middle).unwrap().start_line(), line(2));
    assert_eq!(folds.get(late).unwrap().start_line(), line(4));

    assert_eq!(folds.unfold(middle).unwrap().id(), middle);
    assert_eq!(folds.len(), 2);
    assert!(folds.get(middle).is_none());
    assert_eq!(preserved.len(), 3);
    assert!(preserved.get(middle).is_some());
}

#[test]
fn unfolding_outer_fold_should_locally_reveal_nested_hidden_span() {
    let buffer = buffer("0\n1\n2\n3\n4\n5\n");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    let outer = folds.fold_lines(&snapshot, line_range(0, 5)).unwrap();
    let inner = folds.fold_lines(&snapshot, line_range(1, 4)).unwrap();

    assert_eq!(
        folds
            .derive_hidden_ranges()
            .unwrap()
            .into_iter()
            .map(HiddenRange::lines)
            .collect::<Vec<_>>(),
        vec![line_range(1, 5)]
    );

    folds.unfold(outer).unwrap();

    assert!(folds.get(inner).is_some());
    assert_eq!(
        folds
            .derive_hidden_ranges()
            .unwrap()
            .into_iter()
            .map(HiddenRange::lines)
            .collect::<Vec<_>>(),
        vec![line_range(2, 4)]
    );
    Projection::build(&snapshot, &folds).unwrap();
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
        DisplayMapError::Projection(ProjectionError::VersionMismatch { .. })
    ));
}

#[test]
fn projection_build_should_compress_unfolded_lines_without_reading_each_line() {
    let text = "x\n".repeat(10_000);
    let buffer = buffer(&text);
    let snapshot = buffer.snapshot();
    let folds = FoldSet::new(snapshot.version());
    let projection = Projection::build(&snapshot, &folds).unwrap();

    assert_eq!(projection.line_count(), 10_001);
    assert_eq!(projection.summary_item_count(), 1);
}

#[test]
fn projection_line_map_should_distinguish_text_rows_hidden_rows_and_placeholder_rows() {
    let buffer = buffer("a\nb\nc\nd");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    folds.fold_lines(&snapshot, line_range(1, 3)).unwrap();
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
fn projection_should_derive_shifted_absolute_lines_from_dual_dimension_prefixes() {
    let buffer = buffer("0\n1\n2\n3\n4\n5\n6\n7\n8");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    folds.fold_lines(&snapshot, line_range(1, 4)).unwrap();
    folds.fold_lines(&snapshot, line_range(5, 8)).unwrap();
    let projection = Projection::build(&snapshot, &folds).unwrap();

    let kinds: Vec<_> = projection
        .iter()
        .map(|row| match row.kind() {
            ProjectedLineKind::Text(text) => (Some(text.logical_line()), None, None),
            ProjectedLineKind::Placeholder(placeholder) => (
                None,
                Some(placeholder.anchor_line()),
                Some(placeholder.hidden_lines()),
            ),
        })
        .collect();

    assert_eq!(
        kinds,
        vec![
            (Some(line(0)), None, None),
            (Some(line(1)), None, None),
            (None, Some(line(1)), Some(line_range(2, 4))),
            (Some(line(4)), None, None),
            (Some(line(5)), None, None),
            (None, Some(line(5)), Some(line_range(6, 8))),
            (Some(line(8)), None, None),
        ]
    );
    assert_eq!(
        projection.logical_to_projected(line(8)).unwrap(),
        LogicalProjection::Visible(projected(6))
    );
    assert_eq!(
        projection.logical_to_projected(line(7)).unwrap(),
        LogicalProjection::Hidden {
            anchor_logical_line: line(5),
            anchor_projected_line: projected(4),
        }
    );
}

#[test]
fn projection_point_and_range_mapping_should_return_typed_hidden_and_placeholder_facts() {
    let buffer = buffer("a\nb\nc\nd");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    folds.fold_lines(&snapshot, line_range(1, 3)).unwrap();
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
    folds.fold_lines(&snapshot, line_range(1, 3)).unwrap();
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
