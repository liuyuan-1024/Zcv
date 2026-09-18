//! 有界高亮结果缓存。
//!
//! 高亮是从语法快照派生的可丢弃数据：
//! 缓存由拥有语言状态的 Buffer 持有，随文本版本变化或解析安装整体替换，不进入不可变的 `SyntaxSnapshot`。
//! 容量以条目字节成本为预算，超出后按最近最少使用淘汰（对齐 Zed `ChunkHighlightCache`）。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::HighlightSpan;

/// 高亮缓存总字节预算。
const MAX_HIGHLIGHT_CACHE_BYTES: usize = 10 * 1024 * 1024;

/// 按 chunk 键控的有界高亮结果缓存。
pub struct HighlightCache {
    inner: Mutex<Inner>,
}

struct Inner {
    entries: HashMap<usize, CacheEntry>,
    order: VecDeque<usize>,
    total_bytes: usize,
}

struct CacheEntry {
    spans: Arc<[HighlightSpan]>,
    bytes: usize,
}

impl HighlightCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
                order: VecDeque::new(),
                total_bytes: 0,
            }),
        }
    }

    pub(crate) fn get(&self, chunk_start: usize) -> Option<Arc<[HighlightSpan]>> {
        let mut inner = self.inner.lock().expect("高亮缓存锁不应中毒");
        let spans = inner.entries.get(&chunk_start)?.spans.clone();
        if let Some(index) = inner.order.iter().position(|key| *key == chunk_start) {
            inner.order.remove(index);
        }
        inner.order.push_back(chunk_start);
        Some(spans)
    }

    pub(crate) fn insert(&self, chunk_start: usize, spans: Arc<[HighlightSpan]>) {
        let bytes = spans.len() * size_of::<HighlightSpan>();
        // 单个 chunk 就超过总预算时不缓存，避免一次插入把全部历史条目挤空。
        if bytes > MAX_HIGHLIGHT_CACHE_BYTES {
            return;
        }
        let mut inner = self.inner.lock().expect("高亮缓存锁不应中毒");
        if let Some(previous) = inner
            .entries
            .insert(chunk_start, CacheEntry { spans, bytes })
        {
            inner.total_bytes = inner.total_bytes.saturating_sub(previous.bytes);
            if let Some(index) = inner.order.iter().position(|key| *key == chunk_start) {
                inner.order.remove(index);
            }
        }
        inner.order.push_back(chunk_start);
        inner.total_bytes += bytes;
        while inner.total_bytes > MAX_HIGHLIGHT_CACHE_BYTES {
            let Some(evicted) = inner.order.pop_front() else {
                break;
            };
            if let Some(entry) = inner.entries.remove(&evicted) {
                inner.total_bytes = inner.total_bytes.saturating_sub(entry.bytes);
            }
        }
    }
}

impl Default for HighlightCache {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for HighlightCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HighlightCache")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
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
        cache.insert(4096, spans(per_entry));
        assert!(cache.get(0).is_some(), "两个条目应在预算内");
        cache.insert(8192, spans(per_entry));
        assert!(
            cache.get(4096).is_none(),
            "超出预算时应淘汰最久未使用的条目"
        );
        assert!(cache.get(0).is_some(), "刚访问过的条目应保留");
        assert!(cache.get(8192).is_some());
    }

    #[test]
    fn oversized_chunk_is_not_cached() {
        let cache = HighlightCache::new();
        let oversized = MAX_HIGHLIGHT_CACHE_BYTES / size_of::<HighlightSpan>() + 1;
        cache.insert(0, spans(oversized));
        assert!(cache.get(0).is_none());
    }
}
