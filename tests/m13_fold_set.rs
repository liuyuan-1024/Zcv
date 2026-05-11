//! M13A 机器契约：锁定 FoldRange / FoldSet / HiddenRange 折叠模型语义。
//!
//! 验证范围：
//! - fold / unfold / toggle / unfold all 与 line-based fold;
//! - 嵌套 fold 合法、部分重叠 fold 拒绝;
//! - 编辑后 fold range 跟随；删除命中后的保留 / 塌缩 / 失效策略;
//! - 逻辑行 hidden 查询与 HiddenRange 列表生成;
//! - 版本不匹配的 DeltaEvent 必须原子拒绝。
//!
//! 本文件不涉及投影坐标 / fold placeholder 样式（M13B 起），也不涉及 UI 渲染。

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, Edit, EngineError, FoldError, FoldRange,
    FoldRangeUpdate, FoldSet, FoldToggleOutcome, HiddenRange, Line, LineRange, TextRange,
    TrackedRangeCollapsePolicy, TrackedRangeInvalidationPolicy, TrackedRangeUpdatePolicy,
    Transaction,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn c(value: usize) -> ByteOffset {
    ByteOffset::new(value)
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

fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> zom_engine::DeltaEvent {
    let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
    buffer.apply_transaction(tx).unwrap();
    buffer.last_delta_event().unwrap().clone()
}

#[test]
fn fold_range_binds_to_buffer_version_and_keeps_range() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);
    let id = set.fold(range(2, 5)).unwrap();
    let fold = set.iter().find(|fold| fold.id() == id).unwrap();

    assert_eq!(fold.id(), id);
    assert_eq!(fold.version(), BufferVersion::INITIAL);
    assert_eq!(fold.range(), range(2, 5));
    assert_eq!(
        fold.update_policy(),
        TrackedRangeUpdatePolicy::invalidate_when_fully_deleted()
    );
}

#[test]
fn fold_set_assigns_monotonic_ids_and_normalizes_after_insert() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);

    let outer = set.fold(range(0, 20)).unwrap();
    let middle = set.fold(range(8, 12)).unwrap();
    let inner = set.fold(range(2, 6)).unwrap();

    assert_eq!(outer.get(), 0);
    assert_eq!(middle.get(), 1);
    assert_eq!(inner.get(), 2);
    assert_eq!(set.len(), 3);

    let folds: Vec<TextRange> = set.iter().map(FoldRange::range).collect();
    assert_eq!(folds, vec![range(0, 20), range(2, 6), range(8, 12)]);
}

#[test]
fn fold_idempotent_on_exact_existing_range() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);
    let first = set.fold(range(2, 6)).unwrap();
    let second = set.fold(range(2, 6)).unwrap();
    assert_eq!(first, second);
    assert_eq!(set.len(), 1);
}

#[test]
fn fold_rejects_empty_range() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);
    let err = set.fold(range(3, 3)).unwrap_err();
    assert_eq!(err, FoldError::EmptyRange { range: range(3, 3) });
    assert!(set.is_empty());
}

#[test]
fn fold_allows_nested_ranges() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);
    let outer = set.fold(range(0, 20)).unwrap();
    let inner = set.fold(range(4, 10)).unwrap();
    let deepest = set.fold(range(5, 8)).unwrap();
    assert!(set.get(outer).is_some());
    assert!(set.get(inner).is_some());
    assert!(set.get(deepest).is_some());
    assert_eq!(set.len(), 3);
}

#[test]
fn fold_rejects_partial_overlap() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);
    set.fold(range(2, 8)).unwrap();
    let err = set.fold(range(5, 12)).unwrap_err();
    assert_eq!(
        err,
        FoldError::OverlapWithoutNesting {
            existing: range(2, 8),
            candidate: range(5, 12),
        }
    );
    assert_eq!(set.len(), 1);
}

#[test]
fn unfold_removes_specific_id_only() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);
    let a = set.fold(range(2, 6)).unwrap();
    let b = set.fold(range(8, 12)).unwrap();

    let removed = set.unfold(a).unwrap();
    assert_eq!(removed.id(), a);
    assert_eq!(set.len(), 1);
    assert!(set.get(a).is_none());
    assert!(set.get(b).is_some());

    assert!(set.unfold(a).is_none());
}

#[test]
fn unfold_at_picks_innermost_fold_containing_offset() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);
    let outer = set.fold(range(0, 20)).unwrap();
    let inner = set.fold(range(4, 10)).unwrap();
    let deepest = set.fold(range(5, 8)).unwrap();

    let removed = set.unfold_at(c(6)).unwrap();
    assert_eq!(removed.id(), deepest);
    assert!(set.get(outer).is_some());
    assert!(set.get(inner).is_some());

    assert!(set.unfold_at(c(100)).is_none());
}

#[test]
fn unfold_all_clears_every_fold() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);
    set.fold(range(0, 5)).unwrap();
    set.fold(range(10, 14)).unwrap();
    set.unfold_all();
    assert!(set.is_empty());
}

#[test]
fn toggle_unfolds_existing_exact_range_and_folds_new_range() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);
    let folded = set.toggle(range(2, 6)).unwrap();
    match folded {
        FoldToggleOutcome::Folded(id) => assert!(set.get(id).is_some()),
        FoldToggleOutcome::Unfolded(_) => panic!("expected Folded"),
    }
    assert_eq!(set.len(), 1);

    let unfolded = set.toggle(range(2, 6)).unwrap();
    match unfolded {
        FoldToggleOutcome::Unfolded(_) => assert!(set.is_empty()),
        FoldToggleOutcome::Folded(_) => panic!("expected Unfolded"),
    }
}

#[test]
fn toggle_rejects_partial_overlap_when_adding_new_fold() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);
    set.fold(range(2, 8)).unwrap();
    let err = set.toggle(range(5, 12)).unwrap_err();
    assert!(matches!(err, FoldError::OverlapWithoutNesting { .. }));
}

#[test]
fn fold_lines_converts_line_range_to_char_range() {
    let mut buffer = buffer("aaa\nbbb\nccc\nddd\n");
    let mut set = FoldSet::new(buffer.version());

    let id = set.fold_lines(&buffer, line_range(1, 3)).unwrap();
    let fold = set.get(id).unwrap();
    let start = buffer.line_start(line(1)).unwrap();
    let end = buffer.line_start(line(3)).unwrap();
    assert_eq!(fold.range(), TextRange::new(start, end).unwrap());

    // Apply an unrelated edit to confirm fold survives at version boundary; advance via DeltaEvent.
    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(0), "X".to_string()).unwrap()],
    );
    set.update_through_delta_event(&event).unwrap();
}

#[test]
fn is_line_hidden_reports_lines_inside_fold_excluding_anchor() {
    let buffer = buffer("aaa\nbbb\nccc\nddd\n");
    let mut set = FoldSet::new(buffer.version());

    set.fold_lines(&buffer, line_range(0, 3)).unwrap();

    assert!(!set.is_line_hidden(&buffer, line(0)).unwrap());
    assert!(set.is_line_hidden(&buffer, line(1)).unwrap());
    assert!(set.is_line_hidden(&buffer, line(2)).unwrap());
    assert!(!set.is_line_hidden(&buffer, line(3)).unwrap());
}

#[test]
fn derive_hidden_ranges_merges_overlapping_and_nested_folds() {
    let buffer = buffer("a\nb\nc\nd\ne\nf\ng\nh\n");
    let mut set = FoldSet::new(buffer.version());

    set.fold_lines(&buffer, line_range(0, 4)).unwrap();
    set.fold_lines(&buffer, line_range(1, 3)).unwrap();
    set.fold_lines(&buffer, line_range(5, 8)).unwrap();

    let hidden = set.derive_hidden_ranges(&buffer).unwrap();
    let line_ranges: Vec<LineRange> = hidden.iter().copied().map(HiddenRange::lines).collect();

    assert_eq!(line_ranges, vec![line_range(1, 4), line_range(6, 8)]);
}

#[test]
fn derive_hidden_ranges_skips_intra_line_folds() {
    let buffer = buffer("hello world\n");
    let mut set = FoldSet::new(buffer.version());

    set.fold(range(2, 5)).unwrap();

    let hidden = set.derive_hidden_ranges(&buffer).unwrap();
    assert!(hidden.is_empty());
    assert!(!set.is_line_hidden(&buffer, line(0)).unwrap());
}

#[test]
fn fold_ranges_follow_text_edits_through_delta_event() {
    let mut buffer = buffer("abcdefghij");
    let mut set = FoldSet::new(buffer.version());

    let id = set.fold(range(2, 6)).unwrap();

    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(0), "XYZ".to_string()).unwrap()],
    );
    let updates = set.update_through_delta_event(&event).unwrap();

    assert_eq!(set.version(), buffer.version());
    assert_eq!(
        updates,
        vec![FoldRangeUpdate::Mapped {
            id,
            range: range(5, 9),
            version: buffer.version(),
        }]
    );
    assert_eq!(set.get(id).unwrap().range(), range(5, 9));
}

#[test]
fn fold_invalidates_when_fully_deleted_under_default_policy() {
    let mut buffer = buffer("abcdef");
    let mut set = FoldSet::new(buffer.version());

    let id = set.fold(range(2, 5)).unwrap();

    let event = apply(&mut buffer, vec![Edit::delete(range(2, 5))]);
    let updates = set.update_through_delta_event(&event).unwrap();

    assert_eq!(
        updates,
        vec![FoldRangeUpdate::Invalidated {
            id,
            range: range(2, 2),
            version: buffer.version(),
        }]
    );
    assert!(set.get(id).is_none());
    assert!(set.is_empty());
}

#[test]
fn fold_can_keep_collapsed_range_when_policy_allows_partial_survival() {
    let mut buffer = buffer("abcdef");
    let policy = TrackedRangeUpdatePolicy::new(
        TrackedRangeInvalidationPolicy::Never,
        TrackedRangeCollapsePolicy::Keep,
    );
    let mut set = FoldSet::new(buffer.version());

    let id = set.fold_with_policy(range(2, 5), policy).unwrap();

    let event = apply(&mut buffer, vec![Edit::delete(range(2, 5))]);
    let updates = set.update_through_delta_event(&event).unwrap();

    assert_eq!(
        updates,
        vec![FoldRangeUpdate::Collapsed {
            id,
            range: range(2, 2),
            version: buffer.version(),
        }]
    );
    assert!(set.get(id).is_some());
}

#[test]
fn fold_invalidates_when_touched_by_deletion_under_strict_policy() {
    let mut buffer = buffer("abcdef");
    let policy = TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion();
    let mut set = FoldSet::new(buffer.version());

    let id = set.fold_with_policy(range(2, 5), policy).unwrap();

    let event = apply(&mut buffer, vec![Edit::delete(range(3, 4))]);
    let updates = set.update_through_delta_event(&event).unwrap();

    assert_eq!(updates.len(), 1);
    assert!(updates[0].is_invalidated());
    assert_eq!(updates[0].id(), id);
    assert!(set.get(id).is_none());
}

#[test]
fn update_through_delta_event_rejects_unrelated_event_atomically() {
    let mut buffer = buffer("abcdef");
    let mut set = FoldSet::new(BufferVersion::new(99));
    let id = set.fold(range(1, 3)).unwrap();

    let event = apply(&mut buffer, vec![Edit::delete(range(1, 2))]);
    let err = set.update_through_delta_event(&event).unwrap_err();

    assert_eq!(
        err,
        FoldError::VersionMismatch {
            expected: BufferVersion::INITIAL,
            actual: BufferVersion::new(99),
        }
    );
    assert_eq!(set.version(), BufferVersion::new(99));
    assert_eq!(set.get(id).unwrap().range(), range(1, 3));
}

#[test]
fn normalize_keeps_outer_before_inner_for_same_start() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);
    set.fold(range(2, 6)).unwrap();
    set.fold(range(2, 12)).unwrap();
    set.fold(range(2, 9)).unwrap();

    set.normalize();

    let folds: Vec<TextRange> = set.iter().map(FoldRange::range).collect();
    assert_eq!(folds, vec![range(2, 12), range(2, 9), range(2, 6)]);
}

#[test]
fn fold_error_propagates_through_engine_error_umbrella() {
    let mut set = FoldSet::new(BufferVersion::INITIAL);
    set.fold(range(2, 8)).unwrap();
    let err: EngineError = set.fold(range(5, 12)).unwrap_err().into();
    assert!(matches!(
        err,
        EngineError::Fold(FoldError::OverlapWithoutNesting { .. })
    ));
}
