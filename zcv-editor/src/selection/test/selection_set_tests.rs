use zcv_multi_buffer::MultiBufferRange;
use zcv_text::{Buffer, BufferConfig, Edit, TransactionMetadata};

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

/// 源 Anchor 不能推进到目标快照时，选区解析必须显式失败。
#[test]
fn resolve_reports_anchor_version_failure() {
    let mut buffer = Buffer::from_text("hello".to_owned(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let old = MultiBufferSnapshot::from(buffer.snapshot());
    buffer
        .edit(
            [Edit::insert(MultiBufferOffset::new(5).into(), "!").expect("插入编辑应合法")],
            TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");
    let new = MultiBufferSnapshot::from(buffer.snapshot());
    // 锚点来自更新版本，解析到更旧快照显式失败。
    let anchor = new.anchor_at(MultiBufferOffset::new(3), Affinity::After);
    assert!(
        SelectionSet::caret(anchor).resolve(&old).is_err(),
        "不可解析锚点必须作为版本错误上报，不能返回 caret(ZERO)"
    );
}

/// 选区集合是 Editor 的完整交互状态；一个端点版本失效时不能静默丢弃部分选区。
#[test]
fn resolve_does_not_drop_partially_invalid_selections() {
    let mut buffer = Buffer::from_text("hello".to_owned(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let old = MultiBufferSnapshot::from(buffer.snapshot());
    buffer
        .edit(
            [Edit::insert(MultiBufferOffset::new(5).into(), "!").expect("插入编辑应合法")],
            TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");
    let new = MultiBufferSnapshot::from(buffer.snapshot());
    let unresolvable = new.anchor_at(MultiBufferOffset::new(3), Affinity::After);
    let resolvable = old.anchor_at(MultiBufferOffset::new(1), Affinity::After);
    let set = SelectionSet::from_selections(
        vec![Selection::caret(unresolvable), Selection::caret(resolvable)],
        0,
    );
    assert!(
        set.resolve(&old).is_err(),
        "选区版本失效必须上报，不能借由丢弃 primary 修复状态"
    );
}
