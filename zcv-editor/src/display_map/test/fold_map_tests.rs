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
        let batch = subscription.consume();
        let buffer_edits = buffer_edits_from_batch(&batch);
        self.read(new_snapshot, buffer_edits)
    }
}

/// 测试辅助：按锚点顺序解析当前快照下的第一个折叠。
fn first_fold(snapshot: &FoldSnapshot) -> ResolvedFold {
    snapshot
        .folds
        .iter()
        .find_map(|fold| fold.resolve(&snapshot.input))
        .expect("应存在可解析的折叠")
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

/// 渲染层回写的实测宽度进入渲染描述；相同宽度不产生显示编辑，也不推进快照。
#[test]
fn updating_fold_widths_rewrites_renderer_and_skips_unchanged() {
    let buffer = Buffer::from_text("anchor\nhidden\ntail".to_string(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    let (snapshot, edits) = map.fold_text_range(6, 13).expect("折叠应成功");
    assert!(!edits.is_empty(), "折叠必须产生显示编辑");
    let id = ChunkRendererId::Fold(snapshot.folds.iter().next().expect("应存在折叠").id);
    assert_eq!(snapshot.fold_width(id), None, "初始实测宽度必须为空");

    let (snapshot, edits) = map.write().update_fold_widths([(id, gpui::px(40.))]);
    assert!(!edits.is_empty(), "宽度变化必须产生显示编辑");
    assert_eq!(snapshot.fold_width(id), Some(gpui::px(40.)));

    let (snapshot, edits) = map.write().update_fold_widths([(id, gpui::px(40.))]);
    assert!(edits.is_empty(), "相同宽度不应产生显示编辑");
    assert_eq!(snapshot.fold_width(id), Some(gpui::px(40.)));
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
    let outer = first_fold(&map.snapshot).fold.id;

    assert_eq!(map.snapshot.line_count(), 2);
    let (snapshot, edits) = map.write().unfold_ids([outer]);
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
fn multibyte_adjacent_folds_remain_aligned_after_merge_and_followup_edits() {
    let mut buffer = Buffer::from_text("前甲中乙后".to_string(), BufferConfig::default()).unwrap();
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());

    let prefix_len = "前".len();
    let first_fold_end = "前甲".len();
    let second_fold_start = "前甲中".len();
    let second_fold_end = "前甲中乙".len();
    map.fold_text_range(prefix_len, first_fold_end).unwrap();
    let (before, _) = map
        .fold_text_range(second_fold_start, second_fold_end)
        .unwrap();
    let subscription = buffer.subscribe();

    // 删除两个三字节中文折叠之间的文本，使它们在新快照中相邻并合并。
    buffer
        .edit(
            [Edit::delete(
                text_range(first_fold_end, second_fold_start).into(),
            )],
            TransactionMetadata::default(),
        )
        .unwrap();
    let (merged, _) = map.read_test(&buffer, &subscription);
    assert_eq!(
        merged.row_text(ProjectedLineIndex::new(0)).unwrap(),
        "前⋯后"
    );
    assert_eq!(
        merged.transforms.summary().input.len,
        merged.buffer_snapshot().len_bytes().get(),
        "合并折叠后变换树输入必须仍精确覆盖中文快照"
    );

    let fold_id = ChunkRendererId::Fold(
        merged
            .folds
            .iter()
            .next()
            .expect("合并后仍应保留折叠身份")
            .id,
    );
    let (resized, _) = map.write().update_fold_widths([(fold_id, gpui::px(32.))]);
    assert_eq!(
        resized.transforms.summary().input.len,
        resized.buffer_snapshot().len_bytes().get(),
        "宽度回写不得改变变换树与快照的输入边界"
    );

    buffer
        .edit(
            [Edit::insert(MultiBufferOffset::new(first_fold_end).into(), "界").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let (after, _) = map.read_test(&buffer, &subscription);
    assert_eq!(
        after.row_text(ProjectedLineIndex::new(0)).unwrap(),
        "前⋯界⋯后"
    );
    assert_eq!(
        after.transforms.summary().input.len,
        after.buffer_snapshot().len_bytes().get(),
        "后续中文编辑不得让 suffix 与折叠重建区重叠"
    );
    assert_ne!(before.version(), after.version());
}

#[test]
fn random_multibyte_fold_edit_stress_keeps_transforms_aligned() {
    let mut buffer =
        Buffer::from_text("甲乙丙丁戊己庚辛壬癸".repeat(20), BufferConfig::default()).unwrap();
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    let subscription = buffer.subscribe();
    let mut state = 0x9e37_79b9_u64;
    for _ in 0..4000 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let char_count = buffer.snapshot().len_bytes().get() / 3;
        if char_count > 2 && state.is_multiple_of(5) {
            let start = (state as usize % (char_count - 1)) * 3;
            let end =
                start + ((state.rotate_left(17) as usize % (char_count - start / 3 - 1)) + 1) * 3;
            let _ = map.fold_text_range(start, end);
        } else if char_count > 1 && state.is_multiple_of(3) {
            let start_char = state as usize % (char_count - 1);
            let delete_chars = (state.rotate_left(11) as usize % (char_count - start_char)).max(1);
            let start = start_char * 3;
            let end = (start_char + delete_chars).min(char_count) * 3;
            buffer
                .edit(
                    [Edit::delete(text_range(start, end).into())],
                    TransactionMetadata::default(),
                )
                .unwrap();
            let _ = map.read_test(&buffer, &subscription);
        } else {
            let start = (state as usize % (char_count + 1)) * 3;
            buffer
                .edit(
                    [Edit::insert(MultiBufferOffset::new(start).into(), "界").unwrap()],
                    TransactionMetadata::default(),
                )
                .unwrap();
            let _ = map.read_test(&buffer, &subscription);
        }
    }
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
    // 文本编辑让折叠 Anchor 退化为空范围后，该折叠身份不再有显示意义，随规范化移除。
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
    let fold_range = first_fold(&map.snapshot).text_range;
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
    let moved = first_fold(&snapshot).text_range;
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

/// 折叠入口行查询只解析与请求行范围相交的折叠；滚动帧不随折叠总数增长。
#[test]
fn fold_anchor_lines_in_range_reports_only_intersecting_entry_lines() {
    let text = (0..12)
        .map(|index| format!("line {index}\n"))
        .collect::<String>();
    let buffer = Buffer::from_text(text, BufferConfig::default()).expect("测试 Buffer 应能创建");
    let (mut map, _) = FoldMap::new(buffer.snapshot().into());
    let starts = (0..12)
        .map(|line| {
            map.snapshot
                .buffer_snapshot()
                .line_start_byte(Line::new(line))
                .map(|offset| offset.get())
                .unwrap_or_else(|_| map.snapshot.buffer_snapshot().len_bytes().get())
        })
        .collect::<Vec<_>>();
    map.fold_text_range(starts[2], starts[4]).unwrap();
    map.fold_text_range(starts[8], starts[10]).unwrap();

    let snapshot = map.snapshot.clone();
    // 只返回入口行落在请求范围内的折叠。
    assert_eq!(
        snapshot.fold_anchor_lines_in_range(Line::new(1)..Line::new(5)),
        vec![Line::new(2)]
    );
    assert_eq!(
        snapshot.fold_anchor_lines_in_range(Line::new(7)..Line::new(10)),
        vec![Line::new(8)]
    );
    assert!(
        snapshot
            .fold_anchor_lines_in_range(Line::new(4)..Line::new(7))
            .is_empty()
    );

    // 外层折叠覆盖内层入口行时，内层不再作为独立候选返回。
    let nested_buffer =
        Buffer::from_text("a\nb\nc\nd\ne\nf\ng\n".to_string(), BufferConfig::default())
            .expect("嵌套折叠测试 Buffer 应能创建");
    let (mut nested_map, _) = FoldMap::new(nested_buffer.snapshot().into());
    nested_map.fold_text_range(2, 11).unwrap();
    nested_map.fold_text_range(4, 7).unwrap();
    let nested = nested_map.snapshot.clone();
    assert!(
        nested
            .fold_anchor_lines_in_range(Line::new(2)..Line::new(3))
            .is_empty(),
        "外层折叠已覆盖的行不应再作为独立入口行"
    );
}
