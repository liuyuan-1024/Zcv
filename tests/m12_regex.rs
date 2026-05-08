//! M12C 机器契约：锁定当前 Buffer / Snapshot 内正则搜索和正则替换语义。
//!
//! 本阶段使用 `regex` crate 的非回溯引擎与大小限制作为正则防御；不实现跨文件搜索，
//! 搜索任务取消和真正异步调度留到 M15。

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, Edit, EngineError, RegexSearchOptions,
    RegexSearchResult, SearchError, TextRange,
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
fn regex_search_types_are_public_crate_root_api() {
    fn accepts_options(_: RegexSearchOptions) {}
    fn accepts_result(_: Option<RegexSearchResult>) {}

    accepts_options(RegexSearchOptions::default());
    accepts_result(None);
}

#[test]
fn buffer_regex_search_finds_matches_by_char_range() {
    let buffer = buffer("id:42 id:777 名称:九");

    let result = buffer
        .search_regex(r"\w+:\d+", RegexSearchOptions::default())
        .unwrap();

    assert_eq!(result.version(), BufferVersion::INITIAL);
    assert_eq!(result.pattern(), r"\w+:\d+");
    assert_eq!(
        result.ranges().collect::<Vec<_>>(),
        vec![range(0, 5), range(6, 12)]
    );
    assert_eq!(result.matches()[1].ordinal(), 1);
}

#[test]
fn regex_search_options_support_case_and_range() {
    let buffer = buffer("Alpha beta ALPHA beta");
    let result = buffer
        .search_regex(
            r"alpha",
            RegexSearchOptions::default()
                .case_insensitive()
                .with_range(range(1, 16)),
        )
        .unwrap();

    assert_eq!(result.options().range(), Some(range(1, 16)));
    assert_eq!(result.ranges().collect::<Vec<_>>(), vec![range(11, 16)]);
}

#[test]
fn regex_search_supports_multiline_and_dot_newline_options() {
    let buffer = buffer("aa\nbb\ncc");
    let line_start = buffer
        .search_regex(r"^bb$", RegexSearchOptions::default().with_multi_line(true))
        .unwrap();
    let spanning = buffer
        .search_regex(
            r"aa.*cc",
            RegexSearchOptions::default().with_dot_matches_new_line(true),
        )
        .unwrap();

    assert_eq!(line_start.ranges().collect::<Vec<_>>(), vec![range(3, 5)]);
    assert_eq!(spanning.ranges().collect::<Vec<_>>(), vec![range(0, 8)]);
}

#[test]
fn regex_search_rejects_invalid_pattern_without_panic() {
    let buffer = buffer("abc");

    let err = buffer
        .search_regex("(", RegexSearchOptions::default())
        .unwrap_err();

    assert!(matches!(
        err,
        EngineError::Search(SearchError::InvalidRegex { .. })
    ));
}

#[test]
fn regex_search_can_limit_compiled_regex_size() {
    let buffer = buffer("abc");

    let err = buffer
        .search_regex(r"[a-z]+", RegexSearchOptions::default().with_size_limit(1))
        .unwrap_err();

    assert!(matches!(
        err,
        EngineError::Search(SearchError::InvalidRegex { .. })
    ));
}

#[test]
fn snapshot_regex_search_reads_snapshot_text_and_binds_version() {
    let mut buffer = buffer("item-1 item-2");
    let snapshot = buffer.snapshot();

    buffer.replace(range(0, 6), "changed").unwrap();
    let result = snapshot
        .search_regex(r"item-\d", RegexSearchOptions::default())
        .unwrap();

    assert_eq!(result.version(), BufferVersion::INITIAL);
    assert_eq!(
        result.ranges().collect::<Vec<_>>(),
        vec![range(0, 6), range(7, 13)]
    );
    assert!(result.is_stale(buffer.version()));
}

#[test]
fn regex_result_can_be_mounted_as_tracking_metadata_layer() {
    let buffer = buffer("item-1 item-2");
    let result = buffer
        .search_regex(r"item-\d", RegexSearchOptions::default())
        .unwrap();
    let layer = result.to_metadata_layer().unwrap();

    assert_eq!(layer.version(), buffer.version());
    assert_eq!(layer.len(), 2);
    assert_eq!(layer.as_slice()[0].range(), range(0, 6));
    assert_eq!(layer.as_slice()[0].metadata().query(), r"item-\d");
}

#[test]
fn regex_replace_match_expands_captures_through_transaction_pipeline() {
    let mut buffer = buffer("first=1 second=22");
    let result = buffer
        .search_regex(
            r"(?P<key>\w+)=(?P<value>\d+)",
            RegexSearchOptions::default(),
        )
        .unwrap();

    let (delta, _) = buffer
        .replace_regex_match(&result, 1, "${value}:${key}")
        .unwrap()
        .expect("expected regex replacement");

    assert_eq!(buffer.text(), "first=1 22:second");
    assert_eq!(
        delta.edits.as_slice(),
        &[Edit::replace(range(8, 17), "22:second".to_string())]
    );
    assert_eq!(buffer.history_status().undo_depth, 1);
}

#[test]
fn regex_replace_all_is_one_atomic_transaction_and_supports_undo_redo() {
    let mut buffer = buffer("rgb(1,2,3) rgb(4,5,6)");
    let result = buffer
        .search_regex(r"rgb\((\d+),(\d+),(\d+)\)", RegexSearchOptions::default())
        .unwrap();

    let (delta, _) = buffer
        .replace_all_regex_matches(&result, "#$1-$2-$3")
        .unwrap()
        .expect("expected regex replace all");

    assert_eq!(buffer.text(), "#1-2-3 #4-5-6");
    assert_eq!(delta.edits.as_slice().len(), 2);
    assert_eq!(buffer.history_status().undo_depth, 1);

    buffer.undo().unwrap().expect("expected undo");
    assert_eq!(buffer.text(), "rgb(1,2,3) rgb(4,5,6)");

    buffer.redo().unwrap().expect("expected redo");
    assert_eq!(buffer.text(), "#1-2-3 #4-5-6");
}

#[test]
fn regex_replace_rejects_stale_result_without_mutation() {
    let mut buffer = buffer("item-1 item-2");
    let result = buffer
        .search_regex(r"item-\d", RegexSearchOptions::default())
        .unwrap();

    buffer.insert(c(0), "new ").unwrap();
    let err = buffer.replace_all_regex_matches(&result, "x").unwrap_err();

    assert_eq!(
        err,
        EngineError::Search(SearchError::VersionMismatch {
            expected: BufferVersion::new(1),
            actual: BufferVersion::INITIAL,
        })
    );
    assert_eq!(buffer.text(), "new item-1 item-2");
}

#[test]
fn regex_replace_all_handles_empty_matches_without_looping() {
    let mut buffer = buffer("ab");
    let result = buffer
        .search_regex(r"", RegexSearchOptions::default())
        .unwrap();

    buffer
        .replace_all_regex_matches(&result, "|")
        .unwrap()
        .expect("expected boundary insertions");

    assert_eq!(buffer.text(), "|a|b|");
}
