//! Buffer 上的搜索入口：把异步搜索绑定到当前 BufferVersion。
//!
//! 仅做"`impl Buffer`"层的转发，算法在 [`crate::search`]，线程模型在 [`crate::search_async`]。替换路径单独在 [`super::replace`]。

use crate::{
    RegexSearchOptions, RegexSearchResult, SearchHandle, SearchOptions, SearchResult,
    search_async::{spawn_literal_search, spawn_regex_search},
    storage::TextStorage,
};

use super::Buffer;

impl Buffer {
    /// 启动一次 literal 搜索，立刻返回 [`SearchHandle`]——实际匹配在后台线程进行。
    ///
    /// 通过 handle 可以 `cancel()` / `progress()` / `join()`。drop 即取消。
    pub fn search(&self, query: &str, options: SearchOptions) -> SearchHandle<SearchResult> {
        spawn_literal_search(
            self.storage.snapshot(),
            self.version,
            self.config.clone(),
            query.to_string(),
            options,
        )
    }

    /// 启动一次默认选项（大小写敏感、全文）的 literal 搜索。
    pub fn search_literal(&self, query: &str) -> SearchHandle<SearchResult> {
        self.search(query, SearchOptions::default())
    }

    /// 启动一次 regex 搜索。返回 handle，语义与 `search` 一致。
    pub fn search_regex(
        &self,
        pattern: &str,
        options: RegexSearchOptions,
    ) -> SearchHandle<RegexSearchResult> {
        spawn_regex_search(
            self.storage.snapshot(),
            self.version,
            pattern.to_string(),
            options,
        )
    }
}
