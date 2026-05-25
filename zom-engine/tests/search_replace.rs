use zom_engine::*;

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

#[test]
fn literal_search_should_return_versioned_byte_ranges_with_case_and_range_options() {
    let buffer = buffer("Alpha alpha ALPHA");
    let result = buffer
        .search(
            "alpha",
            SearchOptions::new()
                .case_insensitive()
                .with_range(range(0, 11)),
        )
        .unwrap();

    assert_eq!(result.version(), buffer.version());
    assert_eq!(result.query(), "alpha");
    assert_eq!(
        result.ranges().collect::<Vec<_>>(),
        vec![range(0, 5), range(6, 11)]
    );
    assert_eq!(result.match_at(1).unwrap().ordinal(), 1);
    assert!(!result.is_stale(buffer.version()));
}

#[test]
fn empty_search_query_should_return_specific_error_variant() {
    let buffer = buffer("abc");

    let err = buffer.search_literal("").unwrap_err();

    assert!(matches!(err, EngineError::Search(SearchError::EmptyQuery)));
}

#[test]
fn whole_word_search_should_not_match_inside_identifier() {
    let buffer = buffer("foo food foo_bar foo");
    let result = buffer
        .search("foo", SearchOptions::new().with_whole_word(true))
        .unwrap();

    assert_eq!(
        result.ranges().collect::<Vec<_>>(),
        vec![range(0, 3), range(17, 20)]
    );
}

#[test]
fn search_result_should_remap_forward_and_drop_deleted_matches() {
    let mut buffer = buffer("aa bb aa");
    let result = buffer.search_literal("aa").unwrap();

    buffer.delete(range(0, 2)).unwrap();
    let event = buffer.last_delta_event().unwrap().clone();
    let remapped = result.try_remap(&event).unwrap();

    assert_eq!(remapped.version(), buffer.version());
    assert_eq!(remapped.ranges().collect::<Vec<_>>(), vec![range(4, 6)]);
}

#[test]
fn replace_search_match_should_reject_missing_match_and_preserve_state() {
    let mut buffer = buffer("ab ab");
    let result = buffer.search_literal("ab").unwrap();
    let version = buffer.version();

    let err = buffer.replace_search_match(&result, 9, "x").unwrap_err();

    assert!(matches!(
        err,
        EngineError::Search(SearchError::MatchNotFound { ordinal: 9 })
    ));
    assert_eq!(buffer.text().as_ref(), "ab ab");
    assert_eq!(buffer.version(), version);
}

#[test]
fn replace_all_literal_matches_should_apply_single_atomic_transaction_and_restore_through_history()
{
    let mut buffer = buffer("red blue red");
    let result = buffer.search_literal("red").unwrap();

    let applied = buffer.replace_all_search_matches(&result, "green").unwrap();

    assert!(applied.is_some());
    assert_eq!(buffer.text().as_ref(), "green blue green");
    assert_eq!(buffer.history_status().undo_depth, 1);

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer.text().as_ref(), "red blue red");
    buffer.redo().unwrap().unwrap();
    assert_eq!(buffer.text().as_ref(), "green blue green");
}

#[test]
fn stale_search_result_should_not_drive_replacement_after_state_transition() {
    let mut buffer = buffer("ab ab");
    let result = buffer.search_literal("ab").unwrap();
    buffer.insert(b(0), "x").unwrap();
    let version = buffer.version();

    let err = buffer
        .replace_all_search_matches(&result, "cd")
        .unwrap_err();

    assert!(matches!(
        err,
        EngineError::Search(SearchError::VersionMismatch { expected, actual })
            if expected == version && actual == result.version()
    ));
    assert_eq!(buffer.text().as_ref(), "xab ab");
}

#[test]
fn regex_search_should_respect_options_invalid_patterns_and_haystack_budget() {
    let buffer = buffer("a1\nb22\nc333");
    let result = buffer
        .search_regex(
            r"(?m)^[a-z]\d+",
            RegexSearchOptions::new().with_multi_line(true),
        )
        .unwrap();

    assert_eq!(result.len(), 3);
    assert_eq!(
        result.ranges().collect::<Vec<_>>(),
        vec![range(0, 2), range(3, 6), range(7, 11)]
    );

    let invalid = buffer
        .search_regex("(", RegexSearchOptions::new())
        .unwrap_err();
    assert!(matches!(
        invalid,
        EngineError::Search(SearchError::InvalidRegex { .. })
    ));

    let too_large = buffer
        .search_regex(
            r"\d+",
            RegexSearchOptions::new().with_haystack_byte_limit(1),
        )
        .unwrap_err();
    assert!(matches!(
        too_large,
        EngineError::Search(SearchError::RangeTooLarge { .. })
    ));
}

#[test]
fn regex_replacement_should_expand_captures_for_single_and_all_matches() {
    let mut single = buffer("one=1 two=22");
    let result = single
        .search_regex(r"([a-z]+)=(\d+)", RegexSearchOptions::new())
        .unwrap();
    single.replace_regex_match(&result, 1, "$2:$1").unwrap();
    assert_eq!(single.text().as_ref(), "one=1 22:two");

    let mut all = buffer("one=1 two=22");
    let result = all
        .search_regex(r"([a-z]+)=(\d+)", RegexSearchOptions::new())
        .unwrap();
    all.replace_all_regex_matches(&result, "$1($2)").unwrap();
    assert_eq!(all.text().as_ref(), "one(1) two(22)");
}
