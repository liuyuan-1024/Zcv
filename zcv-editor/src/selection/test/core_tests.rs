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

#[test]
fn selection_contract_should_preserve_direction_and_ordered_range() {
    let reversed = selection(7, 2);

    assert_eq!(reversed.tail(), b(7));
    assert_eq!(reversed.head(), b(2));
    assert!(reversed.reversed());
    assert_eq!(reversed.range(), range(2, 7));
}
