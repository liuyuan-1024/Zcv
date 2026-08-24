//! 基于一次搜索结果在 Buffer 上做事务化替换。
//!
//! 替换路径与 [`super::search`] 分开：search 是只读匹配；replace 必须持有 `&mut Buffer`、走事务。
//! 本模块只承担"把 SearchResult / RegexSearchResult 翻成一个原子 Transaction"，匹配生成在 [`crate::search`]。

use std::sync::Arc;

use super::Buffer;
use crate::{
    Edit, RegexSearchResult, SearchError, SearchResult, TextRange, TextResult, TransactionMetadata,
    TransactionOutcome, TransactionSource,
    search::{SearchResultSet, regex_replacement_for_match, regex_replacements_in_text},
};

impl Buffer {
    /// 替换一次搜索结果中的指定匹配。
    ///
    /// `result` 必须绑定当前 BufferVersion；过期搜索结果会被拒绝，避免用旧坐标改写新文本。
    pub fn replace_search_match(
        &mut self,
        result: &SearchResult,
        ordinal: usize,
        replacement: &str,
    ) -> TextResult<Option<TransactionOutcome>> {
        self.ensure_search_result_current(result)?;

        let search_match = result
            .match_at(ordinal)
            .ok_or(SearchError::MatchNotFound { ordinal })?;

        self.replace_search_ranges([search_match.range()], replacement, "替换搜索匹配")
    }

    /// 将一次搜索结果中的所有匹配作为单个原子 Transaction 替换。
    ///
    /// 返回 `None` 表示没有匹配或所有匹配本身已经等于 replacement，因此不递增版本、
    /// 不写入历史，也不产生 DeltaEvent。
    pub fn replace_all_search_matches(
        &mut self,
        result: &SearchResult,
        replacement: &str,
    ) -> TextResult<Option<TransactionOutcome>> {
        self.ensure_search_result_current(result)?;
        self.replace_search_ranges(
            result
                .matches()
                .iter()
                .map(|search_match| search_match.range()),
            replacement,
            "替换全部搜索匹配",
        )
    }

    /// 替换一次正则搜索结果中的指定匹配，replacement 支持 `$1` / `${name}` 捕获组展开。
    pub fn replace_regex_match(
        &mut self,
        result: &RegexSearchResult,
        ordinal: usize,
        replacement: &str,
    ) -> TextResult<Option<TransactionOutcome>> {
        self.ensure_search_result_current(result)?;

        let Some((range, replacement)) =
            regex_replacement_for_match(&self.storage, result, ordinal, replacement)?
        else {
            return Err(SearchError::MatchNotFound { ordinal }.into());
        };

        self.replace_search_edits([(range, replacement)], "替换正则匹配")
    }

    /// 将一次正则搜索结果中的所有匹配作为单个原子 Transaction 替换。
    ///
    /// replacement 支持 `regex` crate 的 `$1` / `${name}` 捕获组展开语义。
    pub fn replace_all_regex_matches(
        &mut self,
        result: &RegexSearchResult,
        replacement: &str,
    ) -> TextResult<Option<TransactionOutcome>> {
        self.ensure_search_result_current(result)?;
        self.replace_search_edits_fallible(
            regex_replacements_in_text(&self.storage, result, replacement)?,
            "替换全部正则匹配",
        )
    }

    /// 校验搜索结果绑定当前版本，过期时拒绝替换。
    fn ensure_search_result_current<O: Copy>(&self, result: &SearchResultSet<O>) -> TextResult<()> {
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
    ) -> TextResult<Option<TransactionOutcome>> {
        let replacement: Arc<str> = Arc::from(replacement);
        self.replace_search_edits(
            ranges
                .into_iter()
                .map(|range| (range, Arc::clone(&replacement))),
            description,
        )
    }

    fn replace_search_edits<R>(
        &mut self,
        edits: impl IntoIterator<Item = (TextRange, R)>,
        description: &'static str,
    ) -> TextResult<Option<TransactionOutcome>>
    where
        R: Into<Arc<str>>,
    {
        self.replace_search_edits_fallible(edits.into_iter().map(Ok), description)
    }

    fn replace_search_edits_fallible<R>(
        &mut self,
        edits: impl IntoIterator<Item = TextResult<(TextRange, R)>>,
        description: &'static str,
    ) -> TextResult<Option<TransactionOutcome>>
    where
        R: Into<Arc<str>>,
    {
        self.ensure_writable()?;

        let mut tx_edits = Vec::new();

        for edit in edits {
            let (range, replacement) = edit?;
            let replacement: Arc<str> = replacement.into();
            self.validate_range(range)?;
            self.validate_edit_boundary(range.start())?;
            self.validate_edit_boundary(range.end())?;

            if self.slice_text(range)?.as_ref() != replacement.as_ref() {
                tx_edits.push(Edit::replace(range, replacement));
            }
        }

        if tx_edits.is_empty() {
            return Ok(None);
        }

        self.edit(
            tx_edits,
            TransactionMetadata::new(TransactionSource::Programmatic).with_description(description),
        )
        .map(Some)
    }
}
