//! 有界高亮结果缓存。
//!
//! 高亮是从语法快照派生的可丢弃数据：
//! 缓存由拥有语言状态的 Buffer 持有，随文本版本变化或解析安装整体替换，不进入不可变的 `SyntaxSnapshot`。
//! 容量以条目字节成本为预算，超出后按最近最少使用淘汰（对齐 Zed `ChunkHighlightCache`）。

use lru::LruCache;
use std::sync::{Arc, Mutex};

use crate::HighlightSpan;

/// 高亮缓存总字节预算。
const MAX_HIGHLIGHT_CACHE_BYTES: usize = 10 * 1024 * 1024;

/// 按 chunk 键控的有界高亮结果缓存。
pub struct HighlightCache {
    inner: Mutex<Inner>,
}

struct Inner {
    entries: LruCache<usize, CacheEntry>,
    total_bytes: usize,
}

struct CacheEntry {
    spans: Arc<[HighlightSpan]>,
    bytes: usize,
}

// 包含键、值及链表节点的估算成本，使空结果也受同一字节预算约束。
const ENTRY_OVERHEAD_BYTES: usize =
    size_of::<usize>() + size_of::<CacheEntry>() + 4 * size_of::<usize>();

impl HighlightCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: LruCache::unbounded(),
                total_bytes: 0,
            }),
        }
    }

    pub(crate) fn get(&self, chunk_start: usize) -> Option<Arc<[HighlightSpan]>> {
        let mut inner = self.inner.lock().expect("高亮缓存锁不应中毒");
        Some(Arc::clone(&inner.entries.get(&chunk_start)?.spans))
    }

    pub(crate) fn insert(&self, chunk_start: usize, spans: Arc<[HighlightSpan]>) {
        let bytes = spans.len() * size_of::<HighlightSpan>() + ENTRY_OVERHEAD_BYTES;
        // 单个 chunk 就超过总预算时不缓存，避免一次插入把全部历史条目挤空。
        if bytes > MAX_HIGHLIGHT_CACHE_BYTES {
            return;
        }
        let mut inner = self.inner.lock().expect("高亮缓存锁不应中毒");
        if let Some(previous) = inner.entries.put(chunk_start, CacheEntry { spans, bytes }) {
            inner.total_bytes -= previous.bytes;
        }
        inner.total_bytes += bytes;
        while inner.total_bytes > MAX_HIGHLIGHT_CACHE_BYTES {
            let (_, entry) = inner.entries.pop_lru().expect("超出预算的缓存必须包含条目");
            inner.total_bytes -= entry.bytes;
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
#[path = "test/highlight_cache_tests.rs"]
mod tests;
