//! Buffer 搜索入口：把 M12A literal search 绑定到当前 BufferVersion。

use crate::{EngineResult, SearchOptions, SearchResult, search::search_in_text};

use super::Buffer;

impl Buffer {
    /// 在当前 Buffer 文本中执行普通字符串搜索。
    pub fn search(&self, query: &str, options: SearchOptions) -> EngineResult<SearchResult> {
        search_in_text(&self.storage, self.version, &self.config, query, options)
    }

    /// 使用默认选项执行大小写敏感的全文普通字符串搜索。
    pub fn search_literal(&self, query: &str) -> EngineResult<SearchResult> {
        self.search(query, SearchOptions::default())
    }
}
