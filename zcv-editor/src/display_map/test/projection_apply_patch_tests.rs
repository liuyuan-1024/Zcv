//! `Projection::apply_patch` 的差分测试：每条编辑后
//! 「增量推进的 Projection」必须与「按新版本重新 build 的 Projection」字段级相等。
//!
//! 同时验证 Tier 1 分类器的 outcome 是否符合预期：
//! - 行内字节编辑（不改变行数、不改变 fold 结构）→ `Compatible`
//! - 改变行数 / 改变 fold 结构 → `Spliced`

use zcv_engine::*;

use super::super::{
    error::{DisplayMapError, ProjectionError},
    fold::FoldSet,
    projection::{ApplyOutcome, Projection},
};
use super::test_helpers::*;

/// 在 `buffer` 上施加一次编辑，同步推进 folds 与 incremental projection，并对比"增量推进结果"与"按新版本全量 build"是否字段级相等。
fn step_and_diff(
    buffer: &mut Buffer,
    folds: &mut FoldSet,
    incremental: &mut Projection,
    edit: impl FnOnce(&mut Buffer),
) -> ApplyOutcome {
    let subscription = buffer.subscribe();
    let old_snapshot = buffer.snapshot();
    let old_version = buffer.version();
    edit(buffer);
    let changes = subscription.consume();
    let snapshot = buffer.snapshot();

    folds
        .update_through_patch(old_version, buffer.version(), changes.patch(), &snapshot)
        .unwrap();

    let outcome = incremental
        .apply_patch(
            &old_snapshot,
            &snapshot,
            folds,
            old_version,
            buffer.version(),
            changes.patch(),
        )
        .unwrap();

    let fresh = Projection::build(&snapshot, folds).unwrap();
    assert_eq!(
        incremental, &fresh,
        "incremental Projection 与 fresh build 不一致"
    );
    outcome
}

#[test]
fn inline_byte_edit_with_no_folds_should_be_compatible() {
    let mut buffer = buffer("hello world\nlorem ipsum\n");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    let mut projection = Projection::build(&snapshot, &folds).unwrap();

    // 在第二行中插入一个字符：不改变行数、不影响 fold 结构。
    let outcome = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
        buf.insert(b(15), "x").unwrap();
    });

    assert!(
        matches!(outcome, ApplyOutcome::Compatible),
        "outcome = {outcome:?}"
    );
}

#[test]
fn inline_delete_in_no_fold_region_should_be_compatible() {
    let mut buffer = buffer("hello world\nlorem ipsum\n");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    let mut projection = Projection::build(&snapshot, &folds).unwrap();

    let outcome = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
        let r = TextRange::new(b(6), b(11)).unwrap();
        buf.delete(r).unwrap();
    });

    assert!(matches!(outcome, ApplyOutcome::Compatible));
}

#[test]
fn newline_insertion_should_splice_projection_rows() {
    let mut buffer = buffer("hello world\nlorem ipsum\n");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    let mut projection = Projection::build(&snapshot, &folds).unwrap();

    // 行内插入一个换行符 -> 行数+1
    let outcome = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
        buf.insert(b(5), "\n").unwrap();
    });

    assert!(matches!(outcome, ApplyOutcome::Spliced));
}

#[test]
fn newline_deletion_should_splice_projection_rows() {
    let mut buffer = buffer("alpha\nbravo\ncharlie\ndelta");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    let mut projection = Projection::build(&snapshot, &folds).unwrap();

    let outcome = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
        buf.delete(range(11, 12)).unwrap();
    });

    assert_eq!(outcome, ApplyOutcome::Spliced);
}

#[test]
fn net_zero_line_delta_with_two_structural_edits_should_still_splice() {
    let mut buffer = buffer("aa\nbb\ncc\ndd\nee\nff");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    let mut projection = Projection::build(&snapshot, &folds).unwrap();

    let outcome = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
        let transaction = tx(
            buf,
            vec![
                Edit::insert(b(1), "\n").unwrap(),
                Edit::delete(range(11, 12)),
            ],
        );
        buf.apply_transaction(transaction).unwrap();
    });

    assert_eq!(outcome, ApplyOutcome::Spliced);
}

#[test]
fn inline_edit_with_static_fold_should_be_compatible() {
    let mut buffer = buffer("alpha\nbravo\ncharlie\ndelta\necho\n");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    // 折叠 line 1..3，anchor 在 line 0 后；fold 不在编辑区
    folds.fold_lines(&snapshot, line_range(1, 3)).unwrap();
    let mut projection = Projection::build(&snapshot, &folds).unwrap();

    // 在 "echo" 一行的开头插入一个字符；不跨行、不影响 fold
    let outcome = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
        // line 4 起点。
        buf.insert(b(26), "X").unwrap();
    });

    assert!(matches!(outcome, ApplyOutcome::Compatible));
}

#[test]
fn inline_edit_inside_fold_should_keep_projection_topology_compatible() {
    let mut buffer = buffer("anchor\nhidden line\nvisible\n");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    folds.fold_lines(&snapshot, line_range(0, 2)).unwrap();
    let mut projection = Projection::build(&snapshot, &folds).unwrap();
    let outcome = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
        buf.insert(b(8), "much wider ").unwrap();
    });

    assert_eq!(outcome, ApplyOutcome::Compatible);
}

#[test]
fn newline_edit_inside_fold_should_expand_splice_to_placeholder_boundaries() {
    let mut buffer = buffer("anchor\nhidden one\nhidden two\nvisible\n");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    folds.fold_lines(&snapshot, line_range(0, 3)).unwrap();
    let mut projection = Projection::build(&snapshot, &folds).unwrap();

    let outcome = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
        buf.insert(b(13), "\nnew hidden").unwrap();
    });

    assert_eq!(outcome, ApplyOutcome::Spliced);
}

#[test]
fn fold_dropped_by_patch_should_splice_projection_rows() {
    let mut buffer = buffer("alpha\nbravo\ncharlie\ndelta\n");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    // 折叠 line 1..3。
    folds.fold_lines(&snapshot, line_range(1, 3)).unwrap();
    let mut projection = Projection::build(&snapshot, &folds).unwrap();
    let fold = folds.iter().next().unwrap();
    assert_eq!(fold.range().start().get(), 6);
    assert_eq!(fold.range().end().get(), 20);

    // 把整个 fold 字节范围删掉 → 默认 policy `invalidate_when_fully_deleted` 让 fold 失效。
    let outcome = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
        let r = TextRange::new(b(6), b(20)).unwrap();
        buf.delete(r).unwrap();
    });

    assert!(matches!(outcome, ApplyOutcome::Spliced));
    assert_eq!(folds.len(), 0, "fold 应该已被 Patch 失效");
}

#[test]
fn unrelated_manual_fold_change_should_conservatively_rebuild() {
    let mut buffer = buffer("zero\none\ntwo\nthree\nfour\n");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    let mut projection = Projection::build(&snapshot, &folds).unwrap();

    let subscription = buffer.subscribe();
    let old_snapshot = buffer.snapshot();
    let old_version = buffer.version();
    buffer.insert(b(21), "X").unwrap();
    let changes = subscription.consume();
    let new_snapshot = buffer.snapshot();
    folds
        .update_through_patch(
            old_version,
            buffer.version(),
            changes.patch(),
            &new_snapshot,
        )
        .unwrap();
    folds.fold_lines(&new_snapshot, line_range(0, 2)).unwrap();

    let outcome = projection
        .apply_patch(
            &old_snapshot,
            &new_snapshot,
            &folds,
            old_version,
            buffer.version(),
            changes.patch(),
        )
        .unwrap();
    let fresh = Projection::build(&new_snapshot, &folds).unwrap();

    assert_eq!(outcome, ApplyOutcome::Rebuilt);
    assert_eq!(projection, fresh);
}

#[test]
fn sequence_of_inline_edits_should_stay_compatible() {
    let mut buffer = buffer("alpha\nbravo\ncharlie\ndelta\n");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    folds.fold_lines(&snapshot, line_range(1, 3)).unwrap();
    let mut projection = Projection::build(&snapshot, &folds).unwrap();

    let initial_version = projection.version();

    // 连续五次行内插入：每次都应该 Compatible
    for offset in [0usize, 1, 2, 3, 4] {
        let outcome = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
            buf.insert(b(offset), "Y").unwrap();
        });
        assert!(
            matches!(outcome, ApplyOutcome::Compatible),
            "offset {offset} 上 outcome = {outcome:?}"
        );
    }

    // version 必须真的推进了 5 次（即每次 Compatible 也确实改了 version）
    assert_ne!(projection.version(), initial_version);
}

#[test]
fn mixed_compatible_and_spliced_sequence_should_keep_projections_aligned() {
    let mut buffer = buffer("alpha\nbravo\ncharlie\ndelta\necho\n");
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(snapshot.version());
    folds.fold_lines(&snapshot, line_range(1, 2)).unwrap();
    let mut projection = Projection::build(&snapshot, &folds).unwrap();

    // 1. 行内编辑：在 "alpha" 开头插入 "A" → Compatible
    let o1 = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
        buf.insert(b(0), "A").unwrap();
    });
    assert!(matches!(o1, ApplyOutcome::Compatible));

    // 2. 在开头插入换行 → 行数 +1 → Spliced
    let o2 = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
        buf.insert(b(0), "\n").unwrap();
    });
    assert!(matches!(o2, ApplyOutcome::Spliced));

    // 3. 行内删除：不跨行 → Compatible
    let o3 = step_and_diff(&mut buffer, &mut folds, &mut projection, |buf| {
        let r = TextRange::new(b(1), b(2)).unwrap();
        buf.delete(r).unwrap();
    });
    assert!(matches!(o3, ApplyOutcome::Compatible));
}

#[test]
fn version_mismatch_should_be_reported_atomically() {
    let mut buffer = buffer("alpha\nbravo\n");
    let snapshot = buffer.snapshot();
    let folds = FoldSet::new(snapshot.version());
    let mut projection = Projection::build(&snapshot, &folds).unwrap();

    // 推进一次 buffer，但 *不* 推进 folds：让三者版本不一致。
    let subscription = buffer.subscribe();
    let old_version = buffer.version();
    buffer.insert(b(0), "X").unwrap();
    let changes = subscription.consume();
    let new_snapshot = buffer.snapshot();

    // folds 还停在旧版本 → 应该报 ApplyPatchStale
    let err = projection
        .apply_patch(
            &snapshot,
            &new_snapshot,
            &folds,
            old_version,
            buffer.version(),
            changes.patch(),
        )
        .unwrap_err();
    assert!(matches!(
        err,
        DisplayMapError::Projection(ProjectionError::ApplyPatchStale { .. })
    ));

    // 错误是原子的：projection 状态未被修改
    let original_version = snapshot.version();
    assert_eq!(projection.version(), original_version);
}
