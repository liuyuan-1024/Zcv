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
