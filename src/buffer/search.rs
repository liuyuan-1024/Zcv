//! Buffer 搜索与替换入口：把 M12 literal search / replace 绑定到当前 BufferVersion。

use crate::{
    ChangeSet, Delta, Edit, EngineResult, SearchError, SearchOptions, SearchResult, TextRange,
    Transaction, TransactionMetadata, TransactionSource, search::search_in_text,
};

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

    /// 替换一次搜索结果中的指定匹配。
    ///
    /// `result` 必须绑定当前 BufferVersion；过期搜索结果会被拒绝，避免用旧坐标改写新文本。
    pub fn replace_search_match(
        &mut self,
        result: &SearchResult,
        ordinal: usize,
        replacement: &str,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.ensure_search_result_current(result)?;

        let search_match = result
            .match_at(ordinal)
            .ok_or(SearchError::MatchNotFound { ordinal })?;

        self.replace_search_ranges([search_match.range()], replacement, "replace search match")
    }

    /// 将一次搜索结果中的所有匹配作为单个原子 Transaction 替换。
    ///
    /// 返回 `None` 表示没有匹配或所有匹配本身已经等于 replacement，因此不递增版本、
    /// 不写入历史，也不产生 DeltaEvent。
    pub fn replace_all_search_matches(
        &mut self,
        result: &SearchResult,
        replacement: &str,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.ensure_search_result_current(result)?;
        self.replace_search_ranges(
            result
                .matches()
                .iter()
                .map(|search_match| search_match.range()),
            replacement,
            "replace all search matches",
        )
    }

    fn ensure_search_result_current(&self, result: &SearchResult) -> EngineResult<()> {
        if result.version() != self.version {
            return Err(SearchError::VersionMismatch {
                expected: self.version,
                actual: result.version(),
            }
            .into());
        }

        Ok(())
    }

    fn replace_search_ranges(
        &mut self,
        ranges: impl IntoIterator<Item = TextRange>,
        replacement: &str,
        description: &'static str,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.ensure_writable()?;
        self.cancel_composition_before_text_edit()?;

        let mut edits = Vec::new();
        let replacement = replacement.to_string();

        for range in ranges {
            self.validate_range(range)?;
            self.validate_edit_boundary(range.start())?;
            self.validate_edit_boundary(range.end())?;

            if self.slice_text(range)?.as_ref() != replacement.as_str() {
                edits.push(Edit::replace(range, replacement.clone()));
            }
        }

        if edits.is_empty() {
            return Ok(None);
        }

        let tx = Transaction::from_edits(self.version, edits)?.with_metadata(
            TransactionMetadata::new(TransactionSource::Programmatic).with_description(description),
        );

        self.apply_transaction(tx).map(Some)
    }
}
