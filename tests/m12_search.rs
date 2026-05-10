//! M12A 机器契约：锁定当前 Buffer / Snapshot 内普通字符串搜索语义。
//!
//! 本阶段只覆盖 literal search、版本绑定、范围限定和搜索结果 metadata 挂载；
//! 不测试正则、不测试替换、不测试跨文件搜索。

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, CoordinateError, Edit, EngineError,
    MetadataLayerKind, SearchError, SearchMatch, SearchMatchMetadata, SearchOptions, SearchResult,
    TextRange, Transaction, VersionedResultError,
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

fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> zom_engine::DeltaEvent {
    let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
    buffer.apply_transaction(tx).unwrap();
    buffer.last_delta_event().unwrap().clone()
}

#[test]
fn search_types_are_public_crate_root_api() {
    fn accepts_options(_: SearchOptions) {}
    fn accepts_result(_: Option<SearchResult>) {}
    fn accepts_match(_: Option<SearchMatch>) {}
    fn accepts_metadata(_: Option<SearchMatchMetadata>) {}

    accepts_options(SearchOptions::default());
    accepts_result(None);
    accepts_match(None);
    accepts_metadata(None);
}

#[test]
fn buffer_search_finds_literal_matches_by_char_range() {
    let buffer = buffer("éxé xx é");

    let result = buffer.search_literal("é").unwrap();
    let ranges = result.ranges().collect::<Vec<_>>();

    assert_eq!(result.version(), BufferVersion::INITIAL);
    assert_eq!(result.query(), "é");
    assert_eq!(result.len(), 3);
    assert_eq!(ranges, vec![range(0, 1), range(2, 3), range(7, 8)]);
    assert_eq!(result.matches()[1].ordinal(), 1);
}

#[test]
fn search_rejects_empty_query() {
    let buffer = buffer("abc");

    let err = buffer.search_literal("").unwrap_err();

    assert_eq!(err, EngineError::Search(SearchError::EmptyQuery));
}

#[test]
fn case_insensitive_search_is_explicit_option() {
    let buffer = buffer("Alpha alpha ALPHA");

    let sensitive = buffer.search_literal("alpha").unwrap();
    let insensitive = buffer
        .search("alpha", SearchOptions::default().case_insensitive())
        .unwrap();

    assert_eq!(sensitive.ranges().collect::<Vec<_>>(), vec![range(6, 11)]);
    assert_eq!(
        insensitive.ranges().collect::<Vec<_>>(),
        vec![range(0, 5), range(6, 11), range(12, 17)]
    );
}

#[test]
fn whole_word_search_respects_identifier_boundaries() {
    let buffer = buffer("foo foo_bar foo. barfoo foo");
    let result = buffer
        .search("foo", SearchOptions::default().with_whole_word(true))
        .unwrap();

    assert_eq!(
        result.ranges().collect::<Vec<_>>(),
        vec![range(0, 3), range(12, 15), range(24, 27)]
    );
}

#[test]
fn search_supports_multiline_queries() {
    let buffer = buffer("aa\nbb\r\ncc\nbb\r\ncc");

    let result = buffer.search_literal("bb\r\ncc").unwrap();

    assert_eq!(
        result.ranges().collect::<Vec<_>>(),
        vec![range(3, 9), range(10, 16)]
    );
}

#[test]
fn search_can_be_limited_to_a_text_range() {
    let buffer = buffer("one two one two one");
    let options = SearchOptions::default().with_range(range(4, 15));

    let result = buffer.search("one", options).unwrap();

    assert_eq!(result.options().range(), Some(range(4, 15)));
    assert_eq!(result.ranges().collect::<Vec<_>>(), vec![range(8, 11)]);
}

#[test]
fn search_range_is_validated_against_current_text() {
    let buffer = buffer("abc");

    let err = buffer
        .search("a", SearchOptions::default().with_range(range(0, 4)))
        .unwrap_err();

    assert_eq!(
        err,
        EngineError::Coordinate(CoordinateError::OutOfBounds(c(4)))
    );
}

#[test]
fn snapshot_search_reads_snapshot_text_and_binds_snapshot_version() {
    let mut buffer = buffer("before match");
    let snapshot = buffer.snapshot();

    buffer.replace(range(7, 12), "changed").unwrap();
    let result = snapshot.search_literal("match").unwrap();

    assert_eq!(result.version(), BufferVersion::INITIAL);
    assert_eq!(result.ranges().collect::<Vec<_>>(), vec![range(7, 12)]);
    assert!(result.is_stale(buffer.version()));
}

#[test]
fn search_result_can_be_mounted_as_tracking_metadata_layer() {
    let mut buffer = buffer("find me and find me");
    let result = buffer.search_literal("find").unwrap();

    let mut layer = result.to_metadata_layer().unwrap();

    assert_eq!(layer.kind(), &MetadataLayerKind::SearchMatch);
    assert_eq!(layer.version(), buffer.version());
    assert_eq!(layer.len(), 2);
    assert_eq!(layer.as_slice()[0].range(), range(0, 4));
    assert_eq!(layer.as_slice()[0].metadata().ordinal(), 0);
    assert_eq!(layer.as_slice()[0].metadata().query(), "find");

    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(0), "please ".to_string()).unwrap()],
    );
    layer.update_through_delta_event(&event).unwrap();

    assert_eq!(layer.version(), buffer.version());
    assert_eq!(layer.as_slice()[0].range(), range(7, 11));
    assert_eq!(layer.as_slice()[1].range(), range(19, 23));
}

#[test]
fn search_result_metadata_invalidates_match_when_deletion_touches_it() {
    let mut buffer = buffer("find me");
    let result = buffer.search_literal("find").unwrap();
    let mut layer = result.to_metadata_layer().unwrap();

    let event = apply(&mut buffer, vec![Edit::delete(range(1, 3))]);
    let updates = layer.update_through_delta_event(&event).unwrap();

    assert!(matches!(
        updates[0],
        zom_engine::MetadataRangeUpdate::Invalidated { .. }
    ));
    assert!(layer.is_empty());
}

#[test]
fn search_result_try_remap_advances_matches_through_insertion() {
    let mut buffer = buffer("find me and find me");
    let result = buffer.search_literal("find").unwrap();
    assert_eq!(
        result.ranges().collect::<Vec<_>>(),
        vec![range(0, 4), range(12, 16)]
    );

    let event = apply(
        &mut buffer,
        vec![Edit::insert(c(0), "please ".to_string()).unwrap()],
    );
    let remapped = result.try_remap(&event).unwrap();

    assert_eq!(remapped.version(), buffer.version());
    assert!(!remapped.is_stale(buffer.version()));
    assert_eq!(remapped.query(), "find");
    assert_eq!(
        remapped.ranges().collect::<Vec<_>>(),
        vec![range(7, 11), range(19, 23)]
    );
    let matches = remapped.matches();
    assert_eq!(matches[0].ordinal(), 0);
    assert_eq!(matches[1].ordinal(), 1);
}

#[test]
fn search_result_try_remap_drops_match_destroyed_by_deletion_and_renumbers_ordinals() {
    let mut buffer = buffer("find me and find me");
    let result = buffer.search_literal("find").unwrap();
    assert_eq!(result.len(), 2);

    // 删除第一处 "find" 的中间两个字符 -> 第一条命中映射为 Deleted，整条丢弃；
    // 第二条仍是 Mapped，按删除后的 char offset 平移。
    let event = apply(&mut buffer, vec![Edit::delete(range(1, 3))]);
    let remapped = result.try_remap(&event).unwrap();

    assert_eq!(remapped.version(), buffer.version());
    assert_eq!(remapped.len(), 1);
    let only_match = remapped.matches()[0];
    assert_eq!(only_match.ordinal(), 0); // 重排后从 0 开始
    assert_eq!(only_match.range(), range(10, 14));
    assert_eq!(remapped.match_at(0), Some(only_match));
    assert_eq!(remapped.match_at(1), None);
}

#[test]
fn search_result_try_remap_rejects_unrelated_event_atomically() {
    let mut snapshot_buffer = buffer("find me");
    let stale_result = snapshot_buffer.search_literal("find").unwrap();

    // 让 stale_result 与 snapshot_buffer 的版本拉开
    apply(
        &mut snapshot_buffer,
        vec![Edit::insert(c(0), "x".to_string()).unwrap()],
    );

    // 再做一次提交，event.old_version != stale_result.version()
    let event = apply(
        &mut snapshot_buffer,
        vec![Edit::insert(c(0), "y".to_string()).unwrap()],
    );

    // 保留一份 clone 用于失败后断言原值不动
    let original = stale_result.clone();
    let err = stale_result.try_remap(&event).unwrap_err();
    assert!(matches!(
        err,
        EngineError::Versioned(VersionedResultError::VersionMismatch { .. })
    ));
    assert_eq!(original.version(), BufferVersion::INITIAL);
    assert_eq!(original.ranges().collect::<Vec<_>>(), vec![range(0, 4)]);
}

#[test]
fn search_result_discard_if_stale_returns_none_only_on_stale_version() {
    let mut buffer = buffer("hello");
    let fresh = buffer.search_literal("hello").unwrap();
    let v0 = buffer.version();
    assert!(fresh.clone().discard_if_stale(v0).is_some());

    apply(
        &mut buffer,
        vec![Edit::insert(c(0), "x".to_string()).unwrap()],
    );
    assert!(fresh.discard_if_stale(buffer.version()).is_none());
}
