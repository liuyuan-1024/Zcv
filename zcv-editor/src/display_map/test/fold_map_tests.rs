use zcv_text::{Buffer, BufferConfig, Edit, TextSubscription, TransactionMetadata};

use super::super::buffer_edits_from_batch;
use super::super::error::DisplayMapError;
use super::*;

fn text_range(start: usize, end: usize) -> MultiBufferRange {
    MultiBufferRange::new(MultiBufferOffset::new(start), MultiBufferOffset::new(end)).unwrap()
}

impl FoldMap {
    /// 测试辅助：按当前快照把字节范围折叠为组合锚点范围后写入。
    fn fold_text_range(
        &mut self,
        start: usize,
        end: usize,
    ) -> DisplayMapResult<(FoldSnapshot, Vec<FoldEdit>)> {
        let range = {
            let snapshot = self.snapshot.buffer_snapshot();
            snapshot.anchor_at(MultiBufferOffset::new(start), Affinity::Before)
                ..snapshot.anchor_at(MultiBufferOffset::new(end), Affinity::After)
        };
        self.write().fold(range, FoldPlaceholder::default())
    }

    /// 测试辅助：把订阅者批次换算成组合文本编辑，再推进 fold 层。
    fn read_test(
        &mut self,
        buffer: &Buffer,
        subscription: &TextSubscription,
    ) -> (FoldSnapshot, Vec<FoldEdit>) {
        let new_snapshot: MultiBufferSnapshot = buffer.snapshot().into();
        let old_snapshot = self.snapshot.buffer_snapshot().clone();
        let batch = subscription.consume();
        let buffer_edits = buffer_edits_from_batch(&batch, &old_snapshot, &new_snapshot);
        self.read(new_snapshot, buffer_edits)
    }
}

/// 本层字节编辑在旧/新快照上映射出的投影行区间。
fn edit_row_ranges(
    edit: &FoldEdit,
    old: &FoldSnapshot,
    new: &FoldSnapshot,
) -> (Range<usize>, Range<usize>) {
    (
        edit.old.start.to_point(old).row()..edit.old.end.to_point(old).row(),
        edit.new.start.to_point(new).row()..edit.new.end.to_point(new).row(),
    )
}

#[test]
fn projected_kind_rejects_the_end_boundary() {
    let buffer = Buffer::from_text("first\nsecond".to_string(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let (_, snapshot) = FoldMap::new(buffer.snapshot().into());

    assert!(
        snapshot
            .projected_kind(ProjectedLineIndex::new(snapshot.line_count()))
            .is_none()
    );
}

#[test]
fn fold_snapshot_owns_fold_and_transform_trees_and_keeps_old_snapshots_stable() {
    let buffer = Buffer::from_text(
        "anchor\nhidden one\nhidden two\nafter".to_string(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建");
    let (mut map, before) = FoldMap::new(buffer.snapshot().into());
    let (after, edits) = map.fold_text_range(6, 21).unwrap();

    assert_eq!(before.line_count(), 4);
    assert_eq!(after.line_count(), 2);
    assert_eq!(
        before.buffer_snapshot().version(),
        after.buffer_snapshot().version()
    );
    assert_ne!(before.version(), after.version());
    assert_eq!(after.folds.summary().count, 1);
    // 变换树输入必须精确覆盖下层文本，同构段不得被折叠变换吞掉。
    assert_eq!(
        after.transforms.summary().input.len,
        after.buffer_snapshot().len_bytes().get()
    );
    assert!(!edits.is_empty());
    let (old_rows, new_rows) = edit_row_ranges(&edits[0], &before, &after);
    assert_ne!(old_rows, new_rows);
}

#[test]
fn folding_a_middle_range_emits_a_localized_structural_edit() {
    let buffer = Buffer::from_text("a\nb\nc\nd\ne\nf\n".to_string(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let (mut map, before) = FoldMap::new(buffer.snapshot().into());
    let (after, edits) = map.fold_text_range(2, 7).unwrap();

    assert_eq!(after.line_count(), 5);
    let edit = &edits[0];
    let (old_rows, new_rows) = edit_row_ranges(edit, &before, &after);
    assert_ne!(old_rows, new_rows);
    // 被折字节区间在旧投影中覆盖 1..3 行，折叠后只产生占位符段所在的合并行。
    assert_eq!(old_rows, 1..3);
    assert_eq!(new_rows, 1..1);
}

#[test]
fn unfolding_a_middle_fold_restores_only_its_rows() {
    let buffer = Buffer::from_text("a\nb\nc\nd\ne\nf\n".to_string(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    map.fold_text_range(2, 7).unwrap();
    let folded = map.snapshot.clone();
    let (after, edits) = map
        .write()
        .unfold_lines(LineRange::new(Line::new(0), Line::new(7)).unwrap())
        .unwrap();

    assert_eq!(after.line_count(), 7);
    let edit = &edits[0];
    let (old_rows, new_rows) = edit_row_ranges(edit, &folded, &after);
    assert_ne!(old_rows, new_rows);
    assert_eq!(old_rows, 1..1);
    assert_eq!(new_rows, 1..3);
}

#[test]
fn fold_writer_rejects_partial_overlap_but_accepts_nesting() {
    let buffer = Buffer::from_text("abcdef".to_string(), BufferConfig::default()).unwrap();
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    map.fold_text_range(1, 5).unwrap();
    map.fold_text_range(2, 4).unwrap();

    let error = map.fold_text_range(0, 3).unwrap_err();
    assert!(matches!(
        error,
        DisplayMapError::Fold(FoldError::OverlapWithoutNesting { .. })
    ));
    assert_eq!(map.snapshot.folds.summary().count, 2);
}

#[test]
fn unfolding_outer_fold_reveals_the_nested_transform() {
    let buffer = Buffer::from_text("a\nb\nc\nd\ne".to_string(), BufferConfig::default()).unwrap();
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    map.fold_text_range(1, 7).unwrap();
    map.fold_text_range(3, 5).unwrap();
    let outer = map
        .snapshot
        .folds
        .iter()
        .min_by_key(|fold| fold.text_range().start())
        .unwrap()
        .id;

    assert_eq!(map.snapshot.line_count(), 2);
    let (snapshot, edits) = map.write().unfold(outer);
    assert_eq!(snapshot.folds.summary().count, 1);
    assert_eq!(snapshot.line_count(), 4);
    assert!(!edits.is_empty());
}

#[test]
fn inline_edit_inside_a_fold_keeps_a_local_edit() {
    let mut buffer =
        Buffer::from_text("anchor\nhidden\nafter".to_string(), BufferConfig::default()).unwrap();
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    map.fold_text_range(6, 13).unwrap();
    let folded = map.snapshot.clone();
    let subscription = buffer.subscribe();
    buffer
        .edit(
            [Edit::insert(MultiBufferOffset::new(9).into(), "!").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();

    let (snapshot, edits) = map.read_test(&buffer, &subscription);
    assert_eq!(snapshot.folds.summary().count, 1);
    assert_eq!(snapshot.line_count(), 2);
    // 折叠内部插入不改变投影行：失效区间新旧相等（合并行就地重排）。
    assert!(edits.iter().all(|edit| {
        let (old_rows, new_rows) = edit_row_ranges(edit, &folded, &snapshot);
        old_rows == new_rows
    }));
}

#[test]
fn editing_inside_a_fold_remeasures_only_the_merged_row() {
    let mut buffer =
        Buffer::from_text("anchor\nhidden\nafter".to_string(), BufferConfig::default()).unwrap();
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    map.fold_text_range(6, 13).unwrap();
    let folded = map.snapshot.clone();
    let subscription = buffer.subscribe();
    buffer
        .edit(
            [Edit::insert(MultiBufferOffset::new(9).into(), "new\n").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();

    let (snapshot, edits) = map.read_test(&buffer, &subscription);
    // 折叠内部插入整行：隐藏行数随之变化，投影行数不变；只有合并行需要重排。
    assert_eq!(snapshot.line_count(), 2);
    // 占位符输出不跨行：字节编辑映射到合并行内的空区间，缓存失效由改变行覆盖。
    let (old_rows, new_rows) = edit_row_ranges(&edits[0], &folded, &snapshot);
    assert_eq!(old_rows, 0..0);
    assert_eq!(new_rows, 0..0);
}

#[test]
fn newline_edit_inside_a_fold_keeps_logical_lines_consistent() {
    let mut buffer =
        Buffer::from_text("anchor\nhidden\nafter".to_string(), BufferConfig::default()).unwrap();
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    map.fold_text_range(6, 13).unwrap();
    let folded = map.snapshot.clone();
    let subscription = buffer.subscribe();
    buffer
        .edit(
            [Edit::insert(MultiBufferOffset::new(9).into(), "new\n").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();

    let (snapshot, edits) = map.read_test(&buffer, &subscription);
    assert_eq!(
        snapshot.logical_line_count(),
        buffer.snapshot().line_count()
    );
    // fold 变换树已重建，但投影行数未变，发出的编辑只覆盖合并行。
    assert_eq!(snapshot.line_count(), 2);
    let (old_rows, new_rows) = edit_row_ranges(&edits[0], &folded, &snapshot);
    assert_eq!(old_rows, 0..0);
    assert_eq!(new_rows, 0..0);
}

#[test]
fn newline_edit_outside_folds_emits_a_localized_structural_edit() {
    let mut buffer =
        Buffer::from_text("a\nb\nc\nd\ne\nf\n".to_string(), BufferConfig::default()).unwrap();
    let (mut map, before) = FoldMap::new(buffer.snapshot().into());
    let subscription = buffer.subscribe();
    // 在未折叠区域插入换行：只应重排该行附近的 tab 行，而不是整份文档。
    buffer
        .edit(
            [Edit::insert(MultiBufferOffset::new(4).into(), "\n").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();

    let (snapshot, edits) = map.read_test(&buffer, &subscription);
    assert_eq!(snapshot.line_count(), 8);
    let (old_rows, new_rows) = edit_row_ranges(&edits[0], &before, &snapshot);
    assert!(old_rows != new_rows);
    assert_eq!(old_rows, 2..2);
    assert_eq!(new_rows, 2..3);
}

#[test]
fn deleting_folded_text_invalidates_anchor_range() {
    let mut buffer =
        Buffer::from_text("anchor\nhidden\nafter".to_string(), BufferConfig::default()).unwrap();
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    map.fold_text_range(6, 13).unwrap();
    let subscription = buffer.subscribe();
    buffer
        .edit(
            [Edit::delete(text_range(0, "anchor\nhidden\n".len()).into())],
            TransactionMetadata::default(),
        )
        .unwrap();

    let (snapshot, _) = map.read_test(&buffer, &subscription);
    assert_eq!(snapshot.folds.summary().count, 0);
    assert_eq!(
        snapshot.line_count(),
        snapshot.buffer_snapshot().line_count()
    );
}

#[test]
fn merged_row_text_joins_anchor_placeholder_and_close_tail() {
    // 折叠范围 = [anchor 行换行符, 闭合括号前)：anchor 文本、占位符、真实 闭合括号 拼成同一行。
    let buffer = Buffer::from_text(
        "fn b() {\n    2\n}\nrest".to_string(),
        BufferConfig::default(),
    )
    .unwrap();
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    // range = [行 0 换行符(8), 闭合括号(15))。
    let (snapshot, _) = map.fold_text_range(8, 15).unwrap();

    assert_eq!(snapshot.line_count(), 2);
    let text = snapshot.row_text(ProjectedLineIndex::new(0)).unwrap();
    assert_eq!(text.as_ref(), "fn b() {⋯}\n");
    // 段表由变换推导：anchor 文本段 + 占位符段 + 闭合行尾段（含行尾换行符）。
    let segments = snapshot
        .fold_row_segments(ProjectedLineIndex::new(0))
        .unwrap();
    assert_eq!(segments.len(), 3);
    assert_eq!(segments[0].merged_range, 0..8);
    assert_eq!(segments[1].merged_range, 8..11);
    assert_eq!(segments[2].merged_range, 11..12);
}

#[test]
fn fold_boundary_insertions_remain_visible() {
    // Stickiness::Never：折叠起点插入的文本在折叠外（可见），折叠终点插入的文本在折叠内。
    let mut buffer =
        Buffer::from_text("anchor\nhidden\nafter".to_string(), BufferConfig::default()).unwrap();
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    map.fold_text_range(6, 13).unwrap();
    let fold_range = map.snapshot.folds.iter().next().unwrap().text_range();
    // 折叠起点 = anchor 行换行符位置（6）。
    assert_eq!(fold_range.start().get(), 6);
    let subscription = buffer.subscribe();
    buffer
        .edit(
            [Edit::insert(fold_range.start().into(), "X").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let (snapshot, _) = map.read_test(&buffer, &subscription);
    // 起点插入在折叠外：折叠范围随插入右移，anchor 行文本变为 "anchorX"。
    let moved = snapshot.folds.iter().next().unwrap().text_range();
    assert_eq!(moved.start().get(), 7);
    let text = snapshot.row_text(ProjectedLineIndex::new(0)).unwrap();
    assert_eq!(text.as_ref(), "anchorX⋯\n");
}

#[test]
fn edits_on_folded_lines_map_to_anchor_row() {
    let mut buffer =
        Buffer::from_text("anchor\nhidden\nafter".to_string(), BufferConfig::default()).unwrap();
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    map.fold_text_range(6, 13).unwrap();
    let folded = map.snapshot.clone();
    // 编辑落在隐藏行（行 1）：投影失效区间覆盖 anchor 行的合并行（行 0）。
    let subscription = buffer.subscribe();
    buffer
        .edit(
            [Edit::insert(MultiBufferOffset::new(9).into(), "X").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let (snapshot, edits) = map.read_test(&buffer, &subscription);
    let (old_rows, new_rows) = edit_row_ranges(&edits[0], &folded, &snapshot);
    assert_eq!(old_rows, 0..0);
    assert_eq!(new_rows, 0..0);
}

#[test]
fn folded_points_map_through_anchor_in_both_directions() {
    let buffer = Buffer::from_text("a\nb\nc\nd".to_string(), BufferConfig::default()).unwrap();
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    let (snapshot, _) = map.fold_text_range(1, 5).unwrap();

    // 折叠段不产生投影行：行数 = 4 - 2 隐藏 = 2。
    assert_eq!(snapshot.line_count(), 2);
    // 隐藏行按 bias 吸附到合并行（anchor 行 0）的折叠起点/终点列；
    // anchor 行 "a" 内容 1 字符，占位符 1 字符。
    let left = snapshot
        .logical_to_projected_point(
            LogicalPoint::new(Line::new(1), LogicalColumn::ZERO),
            FoldBias::Left,
        )
        .unwrap();
    assert_eq!(
        left,
        ProjectedPoint::new(ProjectedLineIndex::new(0), LogicalColumn::new(1))
    );
    let right = snapshot
        .logical_to_projected_point(
            LogicalPoint::new(Line::new(1), LogicalColumn::ZERO),
            FoldBias::Right,
        )
        .unwrap();
    assert_eq!(
        right,
        ProjectedPoint::new(ProjectedLineIndex::new(0), LogicalColumn::new(2))
    );
    // 投影行 1 是可见文本行（"d"）。
    let text = snapshot
        .projected_line_kind(ProjectedLineIndex::new(1))
        .unwrap();
    assert_eq!(text.logical_line(), Line::new(3));
}
