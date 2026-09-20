use super::*;

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

#[test]
fn domain_errors_should_lift_into_text_error_without_losing_variant() {
    let coordinate: TextError = CoordinateError::OutOfBounds(b(99)).into();
    let edit: TextError = EditError::PayloadTooLarge { size: 9, limit: 3 }.into();
    let transaction: TextError = TransactionError::EmptyTransaction.into();

    assert!(matches!(
        coordinate,
        TextError::Coordinate(CoordinateError::OutOfBounds(offset)) if offset == b(99)
    ));
    assert!(matches!(
        edit,
        TextError::Edit(EditError::PayloadTooLarge { size: 9, limit: 3 })
    ));
    assert!(matches!(
        transaction,
        TextError::Transaction(TransactionError::EmptyTransaction)
    ));
}
