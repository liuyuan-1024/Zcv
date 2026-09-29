use super::*;

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

#[test]
fn anchor_ranges_should_express_boundary_insertion_policy() {
    let range = TextRange::new(b(2), b(5)).unwrap();
    let inside = Anchor::range_inside(BufferVersion::INITIAL, range);
    let outside = Anchor::range_outside(BufferVersion::INITIAL, range);

    assert_eq!(inside.start.affinity(), Affinity::After);
    assert_eq!(inside.end.affinity(), Affinity::Before);
    assert_eq!(outside.start.affinity(), Affinity::Before);
    assert_eq!(outside.end.affinity(), Affinity::After);
}
