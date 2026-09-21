use super::*;
use crate::{BufferConfig, BufferVersion};

#[test]
fn replace_text_publishes_its_actual_incremental_patch() {
    let mut buffer = Buffer::from_text("before\n".to_owned(), BufferConfig::default()).unwrap();
    let subscription = buffer.subscribe();

    buffer.replace_text("after\n".to_owned()).unwrap();

    let changes = subscription.consume();
    assert!(changes.transaction_id().is_some());
    assert_eq!(changes.old_version(), Some(BufferVersion::INITIAL));
    assert_eq!(changes.new_version(), Some(BufferVersion::new(1)));
    assert!(!changes.patch().is_empty());
}
