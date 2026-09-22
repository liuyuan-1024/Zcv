use zcv_text::{
    Buffer, BufferConfig, ByteOffset, Edit, TextRange, TransactionMetadata, WordBoundaryPolicy,
};

use super::*;
use crate::search::error::SearchError;

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap()
}

fn buffer_text(buffer: &Buffer) -> String {
    buffer
        .snapshot()
        .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
        .unwrap()
        .into_text()
        .into_owned()
}

#[test]
fn unified_search_query_dispatches_literal_and_regex_with_the_same_options() {
    let buffer = buffer("Cat catalog cat c42");
    let literal = SearchQuery {
        query: "cat".to_string(),
        case_sensitive: false,
        whole_word: true,
        regex: false,
    }
    .search(&buffer.snapshot(), WordBoundaryPolicy::default())
    .unwrap();
    assert!(matches!(literal, SearchQueryResult::Literal(_)));
    assert_eq!(
        literal.ranges().collect::<Vec<_>>(),
        vec![range(0, 3), range(12, 15)]
    );

    let regex_query = SearchQuery {
        query: r"c\d+".to_string(),
        case_sensitive: false,
        whole_word: false,
        regex: true,
    };
    let prepared = regex_query.prepare().unwrap();
    let regex = prepared
        .search(&buffer.snapshot(), WordBoundaryPolicy::default())
        .unwrap();
    assert!(matches!(regex, SearchQueryResult::Regex(_)));
    assert_eq!(regex.ranges().collect::<Vec<_>>(), vec![range(16, 19)]);
}

#[test]
fn literal_search_should_return_versioned_byte_ranges_with_case_and_range_options() {
    let buffer = buffer("Alpha alpha ALPHA");
    let snapshot = buffer.snapshot();
    let result = search_in_text(
        &snapshot,
        snapshot.version(),
        WordBoundaryPolicy::default(),
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
    let error = SearchQuery {
        query: String::new(),
        ..Default::default()
    }
    .search(&buffer.snapshot(), WordBoundaryPolicy::default())
    .unwrap_err();

    assert!(matches!(error, SearchError::EmptyQuery));
}

#[test]
fn whole_word_search_should_not_match_inside_identifier() {
    let buffer = buffer("foo food foo_bar foo");
    let result = SearchQuery {
        query: "foo".to_string(),
        whole_word: true,
        ..Default::default()
    }
    .search(&buffer.snapshot(), WordBoundaryPolicy::default())
    .unwrap();

    assert_eq!(
        result.ranges().collect::<Vec<_>>(),
        vec![range(0, 3), range(17, 20)]
    );
}

#[test]
fn search_result_should_remap_forward_and_drop_deleted_matches() {
    let mut buffer = buffer("aa bb aa");
    let snapshot = buffer.snapshot();
    let result = search_in_text(
        &snapshot,
        snapshot.version(),
        WordBoundaryPolicy::default(),
        "aa",
        SearchOptions::new(),
    )
    .unwrap();

    let outcome = buffer
        .edit([Edit::delete(range(0, 2))], TransactionMetadata::default())
        .unwrap();
    let remapped = result.try_remap(outcome.event()).unwrap();

    assert_eq!(remapped.version(), buffer.version());
    assert_eq!(remapped.ranges().collect::<Vec<_>>(), vec![range(4, 6)]);
}

#[test]
fn regex_search_should_respect_options_and_reject_invalid_patterns() {
    let buffer = buffer("a1\nb22\nc333");
    let snapshot = buffer.snapshot();
    let pattern = r"(?m)^[a-z]\d+";
    let options = RegexSearchOptions {
        multi_line: true,
        ..RegexSearchOptions::new()
    };
    let regex = build_regex_automata(pattern, options).unwrap();
    let result =
        search_regex_streaming_with_regex(&snapshot, snapshot.version(), pattern, &regex, options)
            .unwrap();

    assert_eq!(result.len(), 3);
    assert_eq!(
        result.ranges().collect::<Vec<_>>(),
        vec![range(0, 2), range(3, 6), range(7, 11)]
    );

    let invalid = build_regex_automata("(", RegexSearchOptions::new()).unwrap_err();
    assert!(matches!(invalid, SearchError::InvalidRegex { .. }));
}

#[test]
fn regex_search_should_not_reject_haystacks_beyond_the_old_8mib_cap() {
    // 旧版有 8 MiB 硬限——超过即 RangeTooLarge 拒绝。放开后应能正常完成物化 + 匹配。
    let chunk = "alpha bravo charlie\n"; // 20 字节
    let target_bytes = 10 * 1024 * 1024;
    let mut text = String::with_capacity(target_bytes + chunk.len());
    while text.len() < target_bytes {
        text.push_str(chunk);
    }
    let buffer = buffer(&text);
    let snapshot = buffer.snapshot();
    let pattern = "bravo";
    let options = RegexSearchOptions::new();
    let regex = build_regex_automata(pattern, options).unwrap();
    let result =
        search_regex_streaming_with_regex(&snapshot, snapshot.version(), pattern, &regex, options)
            .unwrap();

    let expected_hits = text.matches("bravo").count();
    assert_eq!(result.len(), expected_hits);
    assert!(expected_hits > (8 * 1024 * 1024) / chunk.len());
}

#[test]
fn regex_replacement_should_expand_captures() {
    let mut buffer = buffer("one=1 two=22");
    let snapshot = buffer.snapshot();
    let pattern = r"([a-z]+)=(\d+)";
    let options = RegexSearchOptions::new();
    let regex = build_regex_automata(pattern, options).unwrap();
    let result =
        search_regex_streaming_with_regex(&snapshot, snapshot.version(), pattern, &regex, options)
            .unwrap();

    let edits = regex_replacements_in_text(&snapshot, &result, "$1($2)")
        .unwrap()
        .map(|edit| edit.unwrap())
        .map(|(range, replacement)| Edit::replace(range, replacement))
        .collect::<Vec<_>>();
    buffer.edit(edits, TransactionMetadata::default()).unwrap();

    assert_eq!(buffer_text(&buffer), "one(1) two(22)");
}
