use super::*;

#[test]
fn versions_and_transaction_ids_should_advance_until_overflow_boundary() {
    assert_eq!(BufferVersion::INITIAL.next(), Some(BufferVersion::new(1)));
    assert_eq!(TransactionId::INITIAL.next(), Some(TransactionId::new(1)));
    assert_eq!(BufferVersion::new(u64::MAX).next(), None);
    assert_eq!(TransactionId::new(u64::MAX).next(), None);
}
