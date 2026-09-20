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
