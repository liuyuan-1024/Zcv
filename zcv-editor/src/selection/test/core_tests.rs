use super::*;
use zcv_text::{Buffer, BufferConfig, Edit, TransactionMetadata};

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

#[test]
fn nonempty_selection_excludes_insertions_at_both_boundaries() {
    for insertion in [1, 3] {
        let mut buffer = Buffer::from_text("abcd".to_owned(), BufferConfig::default()).unwrap();
        let before = MultiBufferSnapshot::from(buffer.snapshot());
        let anchored = selection(1, 3).anchored(&before);
        buffer
            .edit(
                [Edit::insert(b(insertion).into(), "X").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
        let after = MultiBufferSnapshot::from(buffer.snapshot());
        let resolved = anchored.resolve(&after).unwrap();
        let expected = if insertion == 1 {
            selection(2, 4)
        } else {
            selection(1, 3)
        };
        assert_eq!(resolved, expected);
    }
}
