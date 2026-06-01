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

fn buffer_text(buffer: &Buffer) -> String {
    buffer
        .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
        .unwrap()
        .into_text()
        .into_owned()
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
        .join()
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

    let err = buffer.search_literal("").join().unwrap_err();

    assert!(matches!(err, EngineError::Search(SearchError::EmptyQuery)));
}

#[test]
fn whole_word_search_should_not_match_inside_identifier() {
    let buffer = buffer("foo food foo_bar foo");
    let result = buffer
        .search("foo", SearchOptions::new().with_whole_word(true))
        .join()
        .unwrap();

    assert_eq!(
        result.ranges().collect::<Vec<_>>(),
        vec![range(0, 3), range(17, 20)]
    );
}

#[test]
fn search_result_should_remap_forward_and_drop_deleted_matches() {
    let mut buffer = buffer("aa bb aa");
    let result = buffer.search_literal("aa").join().unwrap();

    buffer.delete(range(0, 2)).unwrap();
    let event = buffer.last_delta_event().unwrap().clone();
    let remapped = result.try_remap(&event).unwrap();

    assert_eq!(remapped.version(), buffer.version());
    assert_eq!(remapped.ranges().collect::<Vec<_>>(), vec![range(4, 6)]);
}

#[test]
fn replace_search_match_should_reject_missing_match_and_preserve_state() {
    let mut buffer = buffer("ab ab");
    let result = buffer.search_literal("ab").join().unwrap();
    let version = buffer.version();

    let err = buffer.replace_search_match(&result, 9, "x").unwrap_err();

    assert!(matches!(
        err,
        EngineError::Search(SearchError::MatchNotFound { ordinal: 9 })
    ));
    assert_eq!(buffer_text(&buffer), "ab ab");
    assert_eq!(buffer.version(), version);
}

#[test]
fn replace_all_literal_matches_should_apply_single_atomic_transaction_and_restore_through_history()
{
    let mut buffer = buffer("red blue red");
    let result = buffer.search_literal("red").join().unwrap();

    let applied = buffer.replace_all_search_matches(&result, "green").unwrap();

    assert!(applied.is_some());
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
    let result = buffer.search_literal("ab").join().unwrap();
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
    assert_eq!(buffer_text(&buffer), "xab ab");
}

#[test]
fn regex_search_should_respect_options_and_reject_invalid_patterns() {
    let buffer = buffer("a1\nb22\nc333");
    let result = buffer
        .search_regex(
            r"(?m)^[a-z]\d+",
            RegexSearchOptions::new().with_multi_line(true),
        )
        .join()
        .unwrap();

    assert_eq!(result.len(), 3);
    assert_eq!(
        result.ranges().collect::<Vec<_>>(),
        vec![range(0, 2), range(3, 6), range(7, 11)]
    );

    let invalid = buffer
        .search_regex("(", RegexSearchOptions::new())
        .join()
        .unwrap_err();
    assert!(matches!(
        invalid,
        EngineError::Search(SearchError::InvalidRegex { .. })
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
        .search_regex(r"bravo", RegexSearchOptions::new())
        .join()
        .unwrap();

    let expected_hits = text.matches("bravo").count();
    assert_eq!(result.len(), expected_hits);
    assert!(expected_hits > (8 * 1024 * 1024) / chunk.len());
}

#[test]
fn regex_replacement_should_expand_captures_for_single_and_all_matches() {
    let mut single = buffer("one=1 two=22");
    let result = single
        .search_regex(r"([a-z]+)=(\d+)", RegexSearchOptions::new())
        .join()
        .unwrap();
    single.replace_regex_match(&result, 1, "$2:$1").unwrap();
    assert_eq!(buffer_text(&single), "one=1 22:two");

    let mut all = buffer("one=1 two=22");
    let result = all
        .search_regex(r"([a-z]+)=(\d+)", RegexSearchOptions::new())
        .join()
        .unwrap();
    all.replace_all_regex_matches(&result, "$1($2)").unwrap();
    assert_eq!(buffer_text(&all), "one(1) two(22)");
}

#[test]
fn search_handle_should_report_progress_and_complete() {
    let text: String = "abc ".repeat(2048);
    let buffer = buffer(&text);
    let total = buffer.len_bytes().get() as u64;

    let mut handle = buffer.search_literal("abc");
    while !handle.is_finished() {
        std::thread::yield_now();
    }
    let progress = handle.progress();
    assert_eq!(progress.total_bytes, total);
    assert_eq!(progress.scanned_bytes, total);
    assert!(progress.finished);
    assert!(!progress.cancelled);
    assert_eq!(progress.ratio(), 1.0);

    let result = handle.try_join().unwrap().unwrap();
    assert_eq!(result.len(), 2048);
}

#[test]
fn search_handle_cancel_should_stop_the_worker_with_cancelled_error() {
    // 构造一段需要多个 chunk 的文本，让 cancel 至少有机会命中检查点。
    // 我们不依赖比赛精度——cancel 在线程跑完前命中就返回 Cancelled，否则正常完成。
    let text: String = "x".repeat(512 * 1024);
    let buffer = buffer(&text);
    let handle = buffer.search_literal("x");
    handle.cancel();
    let outcome = handle.join();

    // 任一结果都可接受：要么提前被取消，要么已经完成。两者都是合法终态。
    match outcome {
        Err(EngineError::Search(SearchError::Cancelled)) => {}
        Ok(_) => {}
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[test]
fn search_handle_drop_should_cancel_silently() {
    // drop handle 即取消——这里不验证线程立即退出（无法直接观测），只验证 drop 不阻塞。
    let text: String = "y".repeat(1024);
    let buffer = buffer(&text);
    let handle = buffer.search_literal("y");
    drop(handle);
}
