use zcv_text::*;
mod common;
use common::*;

#[test]
fn unified_search_query_dispatches_literal_and_regex_with_the_same_options() {
    let buffer = buffer("Cat catalog cat c42");
    let literal = SearchQuery {
        query: "cat".to_string(),
        case_sensitive: false,
        whole_word: true,
        regex: false,
    }
    .search(&buffer.snapshot())
    .unwrap();
    assert!(matches!(literal, SearchQueryResult::Literal(_)));
    assert_eq!(
        literal.ranges().collect::<Vec<_>>(),
        vec![range(0, 3), range(12, 15)]
    );

    let regex = SearchQuery {
        query: r"c\d+".to_string(),
        case_sensitive: false,
        whole_word: false,
        regex: true,
    }
    .search(&buffer.snapshot())
    .unwrap();
    assert!(matches!(regex, SearchQueryResult::Regex(_)));
    assert_eq!(regex.ranges().collect::<Vec<_>>(), vec![range(16, 19)]);
}

#[test]
fn literal_search_should_return_versioned_byte_ranges_with_case_and_range_options() {
    let buffer = buffer("Alpha alpha ALPHA");
    let result = buffer
        .snapshot()
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

    let err = buffer.snapshot().search_literal("").unwrap_err();

    assert!(matches!(err, TextError::Search(SearchError::EmptyQuery)));
}

#[test]
fn whole_word_search_should_not_match_inside_identifier() {
    let buffer = buffer("foo food foo_bar foo");
    let result = buffer
        .snapshot()
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
    let result = buffer.snapshot().search_literal("aa").unwrap();

    let outcome = buffer
        .edit([Edit::delete(range(0, 2))], TransactionMetadata::default())
        .unwrap();
    let remapped = result.try_remap(outcome.event()).unwrap();

    assert_eq!(remapped.version(), buffer.version());
    assert_eq!(remapped.ranges().collect::<Vec<_>>(), vec![range(4, 6)]);
}

#[test]
fn replace_search_match_should_reject_missing_match_and_preserve_state() {
    let mut buffer = buffer("ab ab");
    let result = buffer.snapshot().search_literal("ab").unwrap();
    let version = buffer.version();

    let err = buffer.replace_search_match(&result, 9, "x").unwrap_err();

    assert!(matches!(
        err,
        TextError::Search(SearchError::MatchNotFound { ordinal: 9 })
    ));
    assert_eq!(buffer_text(&buffer), "ab ab");
    assert_eq!(buffer.version(), version);
}

#[test]
fn replace_all_literal_matches_should_apply_single_atomic_transaction_and_restore_through_history()
{
    let mut buffer = buffer("red blue red");
    let result = buffer.snapshot().search_literal("red").unwrap();

    let outcome = buffer
        .replace_all_search_matches(&result, "green")
        .unwrap()
        .unwrap();

    assert_eq!(outcome.event().delta().edits().len(), 2);
    assert!(outcome.history_transaction_id().is_some());
    assert_eq!(buffer_text(&buffer), "green blue green");
    assert_eq!(buffer.history_status().undo_depth, 1);

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "red blue red");
    buffer.redo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "green blue green");
}

#[test]
fn stale_search_result_should_not_drive_replacement_after_state_transition() {
    let mut buffer = buffer("ab ab");
    let result = buffer.snapshot().search_literal("ab").unwrap();
    buffer
        .edit(
            [Edit::insert(b(0), "x").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let version = buffer.version();

    let err = buffer
        .replace_all_search_matches(&result, "cd")
        .unwrap_err();

    assert!(matches!(
        err,
        TextError::Search(SearchError::VersionMismatch { expected, actual })
            if expected == version && actual == result.version()
    ));
    assert_eq!(buffer_text(&buffer), "xab ab");
}

#[test]
fn regex_search_should_respect_options_and_reject_invalid_patterns() {
    let buffer = buffer("a1\nb22\nc333");
    let result = buffer
        .snapshot()
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
        .snapshot()
        .search_regex("(", RegexSearchOptions::new())
        .unwrap_err();
    assert!(matches!(
        invalid,
        TextError::Search(SearchError::InvalidRegex { .. })
    ));
}

#[test]
fn regex_search_should_not_reject_haystacks_beyond_the_old_8mib_cap() {
    // 旧版有 8 MiB 硬限——超过即 RangeTooLarge 拒绝。step A 之后该路径放开。
    // 构造 ~10 MiB 文本验证物化 + 匹配能正常完成。
    let chunk = "alpha bravo charlie\n"; // 20 字节
    let target_bytes = 10 * 1024 * 1024;
    let mut text = String::with_capacity(target_bytes + chunk.len());
    while text.len() < target_bytes {
        text.push_str(chunk);
    }
    let buffer = buffer(&text);

    let result = buffer
        .snapshot()
        .search_regex(r"bravo", RegexSearchOptions::new())
        .unwrap();

    let expected_hits = text.matches("bravo").count();
    assert_eq!(result.len(), expected_hits);
    assert!(expected_hits > (8 * 1024 * 1024) / chunk.len());
}

#[test]
fn regex_replacement_should_expand_captures_for_single_and_all_matches() {
    let mut single = buffer("one=1 two=22");
    let result = single
        .snapshot()
        .search_regex(r"([a-z]+)=(\d+)", RegexSearchOptions::new())
        .unwrap();
    single.replace_regex_match(&result, 1, "$2:$1").unwrap();
    assert_eq!(buffer_text(&single), "one=1 22:two");

    let mut all = buffer("one=1 two=22");
    let result = all
        .snapshot()
        .search_regex(r"([a-z]+)=(\d+)", RegexSearchOptions::new())
        .unwrap();
    all.replace_all_regex_matches(&result, "$1($2)").unwrap();
    assert_eq!(buffer_text(&all), "one(1) two(22)");
}
