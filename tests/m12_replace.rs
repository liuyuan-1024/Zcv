//! M12B 机器契约：锁定当前 Buffer 内搜索结果替换语义。
//!
//! 本阶段只覆盖 literal search 结果上的 replace / replace all；不测试正则替换、
//! 不测试跨文件替换，也不引入 Command 层。

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, Edit, EngineError, SearchError, SearchOptions,
    SelectionSet, TextRange, TransactionSource,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(c(start), c(end)).unwrap()
}

#[test]
fn replace_search_match_replaces_one_match_through_transaction_pipeline() {
    let mut buffer = buffer("one two one");
    let result = buffer.search_literal("one").unwrap();

    let outcome = buffer
        .replace_search_match(&result, 1, "three")
        .unwrap()
        .expect("expected replacement transaction");

    assert_eq!(buffer.text(), "one two three");
    assert_eq!(buffer.version(), BufferVersion::new(1));
    assert_eq!(
        outcome.0.edits.as_slice(),
        &[Edit::replace(range(8, 11), "three".to_string())]
    );
    assert_eq!(buffer.history_status().undo_depth, 1);
    assert_eq!(
        buffer.last_delta_event().unwrap().source,
        TransactionSource::Programmatic
    );
}

#[test]
fn replace_search_match_rejects_missing_ordinal_without_mutation() {
    let mut buffer = buffer("one");
    let result = buffer.search_literal("one").unwrap();

    let err = buffer.replace_search_match(&result, 3, "two").unwrap_err();

    assert_eq!(
        err,
        EngineError::Search(SearchError::MatchNotFound { ordinal: 3 })
    );
    assert_eq!(buffer.text(), "one");
    assert_eq!(buffer.version(), BufferVersion::INITIAL);
    assert!(buffer.last_delta_event().is_none());
}

#[test]
fn replace_all_search_matches_is_one_atomic_transaction() {
    let mut buffer = buffer("cat dog cat dog cat");
    let result = buffer.search_literal("cat").unwrap();

    let (delta, _) = buffer
        .replace_all_search_matches(&result, "fox")
        .unwrap()
        .expect("expected replace all transaction");

    assert_eq!(buffer.text(), "fox dog fox dog fox");
    assert_eq!(buffer.version(), BufferVersion::new(1));
    assert_eq!(delta.old_version, BufferVersion::INITIAL);
    assert_eq!(delta.new_version, BufferVersion::new(1));
    assert_eq!(delta.edits.as_slice().len(), 3);
    assert_eq!(buffer.history_status().undo_depth, 1);

    buffer.undo().unwrap().expect("expected undo");
    assert_eq!(buffer.text(), "cat dog cat dog cat");
    assert_eq!(buffer.version(), BufferVersion::new(2));
    assert!(buffer.can_redo());

    buffer.redo().unwrap().expect("expected redo");
    assert_eq!(buffer.text(), "fox dog fox dog fox");
    assert_eq!(buffer.version(), BufferVersion::new(3));
}

#[test]
fn replace_all_preserves_search_options_and_range_limit() {
    let mut buffer = buffer("Alpha alpha ALPHA alpha");
    let result = buffer
        .search(
            "alpha",
            SearchOptions::default()
                .case_insensitive()
                .with_range(range(6, 17)),
        )
        .unwrap();

    buffer
        .replace_all_search_matches(&result, "beta")
        .unwrap()
        .expect("expected scoped replace all");

    assert_eq!(buffer.text(), "Alpha beta beta alpha");
}

#[test]
fn replace_all_rejects_stale_search_result_without_mutation() {
    let mut buffer = buffer("one one");
    let result = buffer.search_literal("one").unwrap();

    buffer.insert(c(0), "zero ").unwrap();
    let err = buffer
        .replace_all_search_matches(&result, "two")
        .unwrap_err();

    assert_eq!(
        err,
        EngineError::Search(SearchError::VersionMismatch {
            expected: BufferVersion::new(1),
            actual: BufferVersion::INITIAL,
        })
    );
    assert_eq!(buffer.text(), "zero one one");
    assert_eq!(buffer.history_status().undo_depth, 1);
}

#[test]
fn replace_all_with_empty_result_is_noop() {
    let mut buffer = buffer("abc");
    let result = buffer.search_literal("z").unwrap();

    let outcome = buffer.replace_all_search_matches(&result, "x").unwrap();

    assert!(outcome.is_none());
    assert_eq!(buffer.text(), "abc");
    assert_eq!(buffer.version(), BufferVersion::INITIAL);
    assert!(buffer.last_delta_event().is_none());
}

#[test]
fn replace_all_skips_noop_edits_without_creating_empty_transaction() {
    let mut buffer = buffer("foo foo");
    let result = buffer.search_literal("foo").unwrap();

    let outcome = buffer.replace_all_search_matches(&result, "foo").unwrap();

    assert!(outcome.is_none());
    assert_eq!(buffer.version(), BufferVersion::INITIAL);
    assert_eq!(buffer.history_status().undo_depth, 0);
}

#[test]
fn replace_all_restores_selection_through_undo_redo() {
    let mut buffer = buffer("one two one");
    let before = SelectionSet::caret(c(4));
    buffer.set_selection(before.clone()).unwrap();
    let result = buffer.search_literal("one").unwrap();

    buffer
        .replace_all_search_matches(&result, "three")
        .unwrap()
        .expect("expected replace all transaction");
    let after = buffer.selection().clone();

    assert_eq!(buffer.text(), "three two three");
    assert_eq!(after, SelectionSet::caret(c(6)));

    buffer.undo().unwrap().expect("expected undo");
    assert_eq!(buffer.text(), "one two one");
    assert_eq!(buffer.selection(), &before);

    buffer.redo().unwrap().expect("expected redo");
    assert_eq!(buffer.text(), "three two three");
    assert_eq!(buffer.selection(), &after);
}

#[test]
fn replace_search_match_can_replace_line_ending_match() {
    let mut buffer = buffer("a\r\nb");
    let result = buffer.search_literal("\r\n").unwrap();

    let outcome = buffer
        .replace_search_match(&result, 0, "\n")
        .unwrap()
        .expect("expected line ending replacement");

    assert_eq!(buffer.text(), "a\nb");
    assert_eq!(
        outcome.0.edits.as_slice(),
        &[Edit::replace(range(1, 3), "\n".to_string())]
    );
}
