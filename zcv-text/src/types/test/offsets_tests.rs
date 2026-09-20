use super::*;

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

#[test]
fn offsets_checked_and_saturating_arithmetic_should_not_panic_at_usize_edges() {
    assert_eq!(b(4).checked_add(3), Some(b(7)));
    assert_eq!(b(4).checked_sub(5), None);
    assert_eq!(ByteOffset::new(usize::MAX).checked_add(1), None);
    assert_eq!(b(4).saturating_sub(9), ByteOffset::ZERO);
    assert_eq!(
        ByteOffset::new(usize::MAX).saturating_add(1),
        ByteOffset::new(usize::MAX)
    );
    assert_eq!(CharOffset::new(4).checked_sub(5), None);
}
