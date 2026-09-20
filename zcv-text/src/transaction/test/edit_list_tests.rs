use super::*;
use crate::{
    errors::EditError,
    transaction::Edit,
    types::{ByteOffset, TextRange},
};

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

#[test]
fn edit_list_should_sort_adjacent_edits_and_reject_overlap() {
    let later = Edit::replace(range(4, 5), "Y".to_string());
    let earlier = Edit::replace(range(0, 1), "X".to_string());
    let sorted = EditList::new(vec![later, earlier]).unwrap();

    assert_eq!(sorted.as_slice()[0].range(), range(0, 1));
    assert_eq!(sorted.as_slice()[1].range(), range(4, 5));

    let err = EditList::new(vec![
        Edit::replace(range(0, 3), "a".to_string()),
        Edit::replace(range(2, 4), "b".to_string()),
    ])
    .unwrap_err();
    assert!(matches!(err, EditError::OverlappingEdits { .. }));
}
