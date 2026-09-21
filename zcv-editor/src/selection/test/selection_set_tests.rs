use zcv_multi_buffer::MultiBufferRange;

use super::*;

fn b(value: usize) -> MultiBufferOffset {
    MultiBufferOffset::new(value)
}

fn range(start: usize, end: usize) -> MultiBufferRange {
    MultiBufferRange::new(b(start), b(end)).unwrap()
}

fn selection(anchor: usize, head: usize) -> Selection<MultiBufferOffset> {
    Selection::new(b(anchor), b(head))
}

fn caret(offset: usize) -> Selection<MultiBufferOffset> {
    Selection::caret(b(offset))
}

#[test]
fn selection_set_normalization_should_sort_merge_duplicates_and_preserve_primary() {
    let set = SelectionSet::new_with_primary(
        vec![caret(8), selection(4, 2), caret(1), selection(3, 6)],
        1,
    );

    assert_eq!(set.primary_index(), 1);
    assert_eq!(set.primary().range(), range(2, 6));
}

#[test]
fn adjacent_non_empty_selections_do_not_merge_but_caret_touching_does() {
    let adjacent = SelectionSet::new(vec![selection(0, 5), selection(5, 10)]);
    assert_eq!(adjacent.len(), 2, "首尾相接的非空选区不合并");

    let caret_touches_end = SelectionSet::new(vec![selection(0, 5), caret(5)]);
    assert_eq!(caret_touches_end.len(), 1, "光标贴在选区终点时合并");
    assert_eq!(caret_touches_end.primary().range(), range(0, 5));

    let caret_touches_start = SelectionSet::new(vec![caret(5), selection(5, 10)]);
    assert_eq!(caret_touches_start.len(), 1, "光标贴在选区起点时合并");
    assert_eq!(caret_touches_start.primary().range(), range(5, 10));

    let overlapping = SelectionSet::new(vec![selection(0, 5), selection(4, 10)]);
    assert_eq!(overlapping.len(), 1, "重叠选区合并");
    assert_eq!(overlapping.primary().range(), range(0, 10));
}

/// 锚点全部无法映射到目标快照时必须显式返回 None，不得静默兜底为文首 caret。
#[test]
fn resolve_returns_none_instead_of_zero_when_no_anchor_maps() {
    let mut buffer =
        zcv_text::Buffer::from_text("hello".to_owned(), zcv_text::BufferConfig::default())
            .expect("测试 Buffer 应能创建");
    let old = MultiBufferSnapshot::from(buffer.snapshot());
    buffer
        .edit(
            [
                zcv_text::Edit::insert(MultiBufferOffset::new(5).into(), "!")
                    .expect("插入编辑应合法"),
            ],
            zcv_text::TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");
    let new = MultiBufferSnapshot::from(buffer.snapshot());
    // 锚点来自更新版本，解析到更旧快照显式失败。
    let anchor = new.anchor_at(MultiBufferOffset::new(3), Affinity::After);
    assert!(
        SelectionSet::caret(anchor).resolve(&old).is_none(),
        "不可解析锚点必须显式失败，不能返回 caret(ZERO)"
    );
}

/// 部分锚点不可解析时丢弃它们并保留其余，primary 归到存活选区。
#[test]
fn resolve_keeps_resolvable_anchors_when_some_fail() {
    let mut buffer =
        zcv_text::Buffer::from_text("hello".to_owned(), zcv_text::BufferConfig::default())
            .expect("测试 Buffer 应能创建");
    let old = MultiBufferSnapshot::from(buffer.snapshot());
    buffer
        .edit(
            [
                zcv_text::Edit::insert(MultiBufferOffset::new(5).into(), "!")
                    .expect("插入编辑应合法"),
            ],
            zcv_text::TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");
    let new = MultiBufferSnapshot::from(buffer.snapshot());
    let unresolvable = new.anchor_at(MultiBufferOffset::new(3), Affinity::After);
    let resolvable = old.anchor_at(MultiBufferOffset::new(1), Affinity::After);
    let set = SelectionSet::from_selections(
        vec![Selection::caret(unresolvable), Selection::caret(resolvable)],
        0,
    );
    let resolved = set.resolve(&old).expect("仍有可解析锚点时必须保留");
    assert_eq!(resolved.as_slice().len(), 1);
    assert_eq!(resolved.primary().head(), MultiBufferOffset::new(1));
}
