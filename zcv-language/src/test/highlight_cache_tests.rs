use super::*;

fn spans(count: usize) -> Arc<[HighlightSpan]> {
    Arc::from(
        (0..count)
            .map(|index| HighlightSpan {
                range: index..index + 1,
                capture: 0,
            })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    )
}

#[test]
fn cache_returns_inserted_chunks() {
    let cache = HighlightCache::new();
    cache.insert(0, spans(2));
    assert_eq!(cache.get(0).map(|spans| spans.len()), Some(2));
}

#[test]
fn cache_is_bounded_by_byte_budget() {
    let cache = HighlightCache::new();
    // 每个条目约占三分之一预算：插入第三个时只淘汰最久未使用的一个。
    let per_entry = MAX_HIGHLIGHT_CACHE_BYTES / 3 / size_of::<HighlightSpan>() + 1;
    cache.insert(0, spans(per_entry));
    cache.insert(1, spans(per_entry));
    assert!(cache.get(0).is_some(), "两个条目应在预算内");
    cache.insert(2, spans(per_entry));
    assert!(cache.get(1).is_none(), "超出预算时应淘汰最久未使用的条目");
    assert!(cache.get(0).is_some(), "刚访问过的条目应保留");
    assert!(cache.get(2).is_some());
}

#[test]
fn oversized_chunk_is_not_cached() {
    let cache = HighlightCache::new();
    let oversized = MAX_HIGHLIGHT_CACHE_BYTES / size_of::<HighlightSpan>() + 1;
    cache.insert(0, spans(oversized));
    assert!(cache.get(0).is_none());
}

#[test]
fn empty_chunks_participate_in_eviction() {
    let cache = HighlightCache::new();
    let count = (MAX_HIGHLIGHT_CACHE_BYTES - ENTRY_OVERHEAD_BYTES) / size_of::<HighlightSpan>();
    cache.insert(0, spans(count));
    for chunk in 1..=3 {
        cache.insert(chunk, Arc::from([]));
    }
    assert!(cache.get(0).is_none(), "空高亮条目也必须消耗缓存预算");
    assert_eq!(cache.get(1).unwrap().len(), 0);
}

#[test]
fn replacing_chunk_releases_previous_cost() {
    let cache = HighlightCache::new();
    let count = MAX_HIGHLIGHT_CACHE_BYTES / 2 / size_of::<HighlightSpan>();
    cache.insert(0, spans(count));
    cache.insert(0, Arc::from([]));
    cache.insert(1, spans(count));
    assert!(cache.get(0).is_some(), "替换后旧跨度成本必须释放");
    assert!(cache.get(1).is_some());
}
