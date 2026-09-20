use super::*;
use crate::{BufferConfig, BufferVersion};

#[test]
fn reset_marks_text_changes_as_a_reset() {
    let mut buffer = Buffer::from_text("before\n".to_owned(), BufferConfig::default()).unwrap();
    let subscription = buffer.subscribe();

    buffer.reset("after\n".to_owned()).unwrap();

    let changes = subscription.consume();
    assert!(changes.requires_reset());
    assert!(changes.transaction_id().is_some());
    assert_eq!(changes.old_version(), Some(BufferVersion::INITIAL));
    assert_eq!(changes.new_version(), Some(BufferVersion::new(1)));
    assert!(!changes.patch().is_empty());
}
