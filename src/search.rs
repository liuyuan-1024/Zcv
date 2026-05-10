//! 当前 Buffer / Snapshot 内的 literal search 与 regex search 契约。
//!
//! 本模块只实现单 Buffer 文本搜索结果模型、纯字符串匹配和正则匹配；不做跨文件索引，
//! 也不承担 UI 高亮语义。

use crate::{
    BufferConfig, BufferVersion, CharOffset, CoordinateError, EngineResult, MetadataLayer,
    MetadataLayerKind, MetadataRangeSpec, SearchError, Stickiness, TextRange, storage::TextRead,
    tracking::TrackedRangeUpdatePolicy,
};
use regex::{Regex, RegexBuilder};

/// 普通字符串搜索选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    range: Option<TextRange>,
    case_sensitive: bool,
    whole_word: bool,
}

impl SearchOptions {
    pub const fn new() -> Self {
        Self {
            range: None,
            case_sensitive: true,
            whole_word: false,
        }
    }

    pub const fn with_range(mut self, range: TextRange) -> Self {
        self.range = Some(range);
        self
    }

    pub const fn without_range(mut self) -> Self {
        self.range = None;
        self
    }

    pub const fn with_case_sensitive(mut self, case_sensitive: bool) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    pub const fn case_insensitive(self) -> Self {
        self.with_case_sensitive(false)
    }

    pub const fn with_whole_word(mut self, whole_word: bool) -> Self {
        self.whole_word = whole_word;
        self
    }

    pub const fn range(self) -> Option<TextRange> {
        self.range
    }

    pub const fn is_case_sensitive(self) -> bool {
        self.case_sensitive
    }

    pub const fn is_whole_word(self) -> bool {
        self.whole_word
    }
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// 正则搜索选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegexSearchOptions {
    range: Option<TextRange>,
    case_sensitive: bool,
    multi_line: bool,
    dot_matches_new_line: bool,
    size_limit: usize,
    dfa_size_limit: usize,
}

impl RegexSearchOptions {
    pub const fn new() -> Self {
        Self {
            range: None,
            case_sensitive: true,
            multi_line: false,
            dot_matches_new_line: false,
            size_limit: 10 * 1024 * 1024,
            dfa_size_limit: 2 * 1024 * 1024,
        }
    }

    pub const fn with_range(mut self, range: TextRange) -> Self {
        self.range = Some(range);
        self
    }

    pub const fn without_range(mut self) -> Self {
        self.range = None;
        self
    }

    pub const fn with_case_sensitive(mut self, case_sensitive: bool) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    pub const fn case_insensitive(self) -> Self {
        self.with_case_sensitive(false)
    }

    pub const fn with_multi_line(mut self, multi_line: bool) -> Self {
        self.multi_line = multi_line;
        self
    }

    pub const fn with_dot_matches_new_line(mut self, dot_matches_new_line: bool) -> Self {
        self.dot_matches_new_line = dot_matches_new_line;
        self
    }

    pub const fn with_size_limit(mut self, size_limit: usize) -> Self {
        self.size_limit = size_limit;
        self
    }

    pub const fn with_dfa_size_limit(mut self, dfa_size_limit: usize) -> Self {
        self.dfa_size_limit = dfa_size_limit;
        self
    }

    pub const fn range(self) -> Option<TextRange> {
        self.range
    }

    pub const fn is_case_sensitive(self) -> bool {
        self.case_sensitive
    }

    pub const fn is_multi_line(self) -> bool {
        self.multi_line
    }

    pub const fn dot_matches_new_line(self) -> bool {
        self.dot_matches_new_line
    }

    pub const fn size_limit(self) -> usize {
        self.size_limit
    }

    pub const fn dfa_size_limit(self) -> usize {
        self.dfa_size_limit
    }
}

impl Default for RegexSearchOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// 单个搜索匹配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SearchMatch {
    ordinal: usize,
    range: TextRange,
}

impl SearchMatch {
    pub(crate) const fn new(ordinal: usize, range: TextRange) -> Self {
        Self { ordinal, range }
    }

    pub const fn ordinal(self) -> usize {
        self.ordinal
    }

    pub const fn range(self) -> TextRange {
        self.range
    }
}

/// 挂载到 `MetadataLayerKind::SearchMatch` 时使用的轻量 payload。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SearchMatchMetadata {
    ordinal: usize,
    query: String,
}

impl SearchMatchMetadata {
    pub fn new(ordinal: usize, query: impl Into<String>) -> Self {
        Self {
            ordinal,
            query: query.into(),
        }
    }

    pub fn ordinal(&self) -> usize {
        self.ordinal
    }

    pub fn query(&self) -> &str {
        &self.query
    }
}

/// 一次搜索结果，绑定被搜索文本的 `BufferVersion`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    version: BufferVersion,
    query: String,
    options: SearchOptions,
    matches: Vec<SearchMatch>,
}

impl SearchResult {
    pub(crate) fn new(
        version: BufferVersion,
        query: String,
        options: SearchOptions,
        matches: Vec<SearchMatch>,
    ) -> Self {
        Self {
            version,
            query,
            options,
            matches,
        }
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn options(&self) -> SearchOptions {
        self.options
    }

    pub fn matches(&self) -> &[SearchMatch] {
        &self.matches
    }

    pub fn match_at(&self, ordinal: usize) -> Option<SearchMatch> {
        self.matches.get(ordinal).copied()
    }

    pub fn ranges(&self) -> impl Iterator<Item = TextRange> + '_ {
        self.matches.iter().map(|search_match| search_match.range())
    }

    pub fn len(&self) -> usize {
        self.matches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    pub fn is_stale(&self, current_version: BufferVersion) -> bool {
        self.version != current_version
    }

    /// 把搜索结果转换为可跟随文本变化的 SearchMatch metadata layer。
    pub fn to_metadata_layer(&self) -> EngineResult<MetadataLayer<SearchMatchMetadata>> {
        let ranges = self.matches.iter().map(|search_match| {
            MetadataRangeSpec::new(
                search_match.range(),
                SearchMatchMetadata::new(search_match.ordinal(), self.query.clone()),
            )
            .with_stickiness(Stickiness::Never)
            .with_update_policy(TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion())
        });

        let mut layer = MetadataLayer::with_kind(MetadataLayerKind::SearchMatch, self.version);
        layer.replace_all_with_options(self.version, ranges)?;
        Ok(layer)
    }
}

/// 一次正则搜索结果，绑定被搜索文本的 `BufferVersion`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexSearchResult {
    version: BufferVersion,
    pattern: String,
    options: RegexSearchOptions,
    matches: Vec<SearchMatch>,
}

impl RegexSearchResult {
    pub(crate) fn new(
        version: BufferVersion,
        pattern: String,
        options: RegexSearchOptions,
        matches: Vec<SearchMatch>,
    ) -> Self {
        Self {
            version,
            pattern,
            options,
            matches,
        }
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn options(&self) -> RegexSearchOptions {
        self.options
    }

    pub fn matches(&self) -> &[SearchMatch] {
        &self.matches
    }

    pub fn match_at(&self, ordinal: usize) -> Option<SearchMatch> {
        self.matches.get(ordinal).copied()
    }

    pub fn ranges(&self) -> impl Iterator<Item = TextRange> + '_ {
        self.matches.iter().map(|search_match| search_match.range())
    }

    pub fn len(&self) -> usize {
        self.matches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    pub fn is_stale(&self, current_version: BufferVersion) -> bool {
        self.version != current_version
    }

    /// 把正则搜索结果转换为可跟随文本变化的 SearchMatch metadata layer。
    pub fn to_metadata_layer(&self) -> EngineResult<MetadataLayer<SearchMatchMetadata>> {
        let ranges = self.matches.iter().map(|search_match| {
            MetadataRangeSpec::new(
                search_match.range(),
                SearchMatchMetadata::new(search_match.ordinal(), self.pattern.clone()),
            )
            .with_stickiness(Stickiness::Never)
            .with_update_policy(TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion())
        });

        let mut layer = MetadataLayer::with_kind(MetadataLayerKind::SearchMatch, self.version);
        layer.replace_all_with_options(self.version, ranges)?;
        Ok(layer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FoldedText {
    text: String,
    byte_boundaries_to_char_offsets: Vec<(usize, usize)>,
}

pub(crate) fn search_in_text<T: TextRead>(
    storage: &T,
    version: BufferVersion,
    config: &BufferConfig,
    query: &str,
    options: SearchOptions,
) -> EngineResult<SearchResult> {
    if query.is_empty() {
        return Err(SearchError::EmptyQuery.into());
    }

    let search_range = options
        .range()
        .unwrap_or_else(|| text_range(CharOffset::ZERO, storage.len_chars()));

    validate_search_range(storage, search_range)?;

    let base_offset = search_range.start().get();
    let haystack = storage.slice_text(search_range)?;
    let matches = if options.is_case_sensitive() {
        find_case_sensitive_matches(
            storage,
            config,
            haystack.as_ref(),
            base_offset,
            query,
            options,
        )
    } else {
        find_case_insensitive_matches(
            storage,
            config,
            haystack.as_ref(),
            base_offset,
            query,
            options,
        )
    }?;

    Ok(SearchResult::new(
        version,
        query.to_string(),
        options,
        matches,
    ))
}

pub(crate) fn search_regex_in_text<T: TextRead>(
    storage: &T,
    version: BufferVersion,
    pattern: &str,
    options: RegexSearchOptions,
) -> EngineResult<RegexSearchResult> {
    let regex = build_regex(pattern, options)?;
    let search_range = options
        .range()
        .unwrap_or_else(|| text_range(CharOffset::ZERO, storage.len_chars()));

    validate_search_range(storage, search_range)?;

    let base_offset = search_range.start().get();
    let haystack = storage.slice_text(search_range)?;
    let matches = regex
        .find_iter(haystack.as_ref())
        .enumerate()
        .map(|(ordinal, regex_match)| {
            let start = base_offset + char_count_until(haystack.as_ref(), regex_match.start());
            let end = base_offset + char_count_until(haystack.as_ref(), regex_match.end());
            SearchMatch::new(
                ordinal,
                text_range(CharOffset::new(start), CharOffset::new(end)),
            )
        })
        .collect();

    Ok(RegexSearchResult::new(
        version,
        pattern.to_string(),
        options,
        matches,
    ))
}

pub(crate) fn regex_replacements_in_text<T: TextRead>(
    storage: &T,
    result: &RegexSearchResult,
    replacement: &str,
) -> EngineResult<Vec<(TextRange, String)>> {
    let regex = build_regex(result.pattern(), result.options())?;
    let search_range = result
        .options()
        .range()
        .unwrap_or_else(|| text_range(CharOffset::ZERO, storage.len_chars()));

    validate_search_range(storage, search_range)?;

    let base_offset = search_range.start().get();
    let haystack = storage.slice_text(search_range)?;
    let mut replacements = Vec::new();

    for captures in regex.captures_iter(haystack.as_ref()) {
        let Some(regex_match) = captures.get(0) else {
            continue;
        };
        let start = base_offset + char_count_until(haystack.as_ref(), regex_match.start());
        let end = base_offset + char_count_until(haystack.as_ref(), regex_match.end());
        let mut expanded = String::new();
        captures.expand(replacement, &mut expanded);
        replacements.push((
            text_range(CharOffset::new(start), CharOffset::new(end)),
            expanded,
        ));
    }

    Ok(replacements)
}

pub(crate) fn regex_replacement_for_match<T: TextRead>(
    storage: &T,
    result: &RegexSearchResult,
    ordinal: usize,
    replacement: &str,
) -> EngineResult<Option<(TextRange, String)>> {
    Ok(regex_replacements_in_text(storage, result, replacement)?
        .into_iter()
        .nth(ordinal))
}

fn build_regex(pattern: &str, options: RegexSearchOptions) -> EngineResult<Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(!options.is_case_sensitive())
        .multi_line(options.is_multi_line())
        .dot_matches_new_line(options.dot_matches_new_line())
        .size_limit(options.size_limit())
        .dfa_size_limit(options.dfa_size_limit())
        .build()
        .map_err(|error| {
            SearchError::InvalidRegex {
                pattern: pattern.to_string(),
                message: error.to_string(),
            }
            .into()
        })
}

fn validate_search_range<T: TextRead>(storage: &T, range: TextRange) -> EngineResult<()> {
    if range.end() > storage.len_chars() {
        return Err(CoordinateError::OutOfBounds(range.end()).into());
    }

    Ok(())
}

fn find_case_sensitive_matches<T: TextRead>(
    storage: &T,
    config: &BufferConfig,
    haystack: &str,
    base_offset: usize,
    query: &str,
    options: SearchOptions,
) -> EngineResult<Vec<SearchMatch>> {
    let mut matches = Vec::new();
    let mut search_from = 0usize;

    while search_from <= haystack.len() {
        let Some(relative_byte_start) = haystack[search_from..].find(query) else {
            break;
        };
        let byte_start = search_from + relative_byte_start;
        let byte_end = byte_start + query.len();
        let start = base_offset + char_count_until(haystack, byte_start);
        let end = base_offset + char_count_until(haystack, byte_end);
        let range = text_range(CharOffset::new(start), CharOffset::new(end));

        if accepts_match(storage, config, range, options)? {
            matches.push(SearchMatch::new(matches.len(), range));
            search_from = byte_end;
        } else {
            search_from = next_char_boundary(haystack, byte_start);
        }
    }

    Ok(matches)
}

fn find_case_insensitive_matches<T: TextRead>(
    storage: &T,
    config: &BufferConfig,
    haystack: &str,
    base_offset: usize,
    query: &str,
    options: SearchOptions,
) -> EngineResult<Vec<SearchMatch>> {
    let folded_haystack = fold_text_with_char_boundaries(haystack);
    let folded_query = query.to_lowercase();
    let mut matches = Vec::new();
    let mut search_from = 0usize;

    while search_from <= folded_haystack.text.len() {
        let Some(relative_byte_start) = folded_haystack.text[search_from..].find(&folded_query)
        else {
            break;
        };
        let byte_start = search_from + relative_byte_start;
        let byte_end = byte_start + folded_query.len();

        let Some(relative_start) = folded_haystack.char_offset_for_byte_boundary(byte_start) else {
            search_from = next_char_boundary(&folded_haystack.text, byte_start);
            continue;
        };
        let Some(relative_end) = folded_haystack.char_offset_for_byte_boundary(byte_end) else {
            search_from = next_char_boundary(&folded_haystack.text, byte_start);
            continue;
        };

        let range = text_range(
            CharOffset::new(base_offset + relative_start),
            CharOffset::new(base_offset + relative_end),
        );

        if accepts_match(storage, config, range, options)? {
            matches.push(SearchMatch::new(matches.len(), range));
            search_from = byte_end;
        } else {
            search_from = next_char_boundary(&folded_haystack.text, byte_start);
        }
    }

    Ok(matches)
}

fn accepts_match<T: TextRead>(
    storage: &T,
    config: &BufferConfig,
    range: TextRange,
    options: SearchOptions,
) -> EngineResult<bool> {
    if !options.is_whole_word() {
        return Ok(true);
    }

    let before = range.start().checked_sub(1).and_then(|offset| {
        if range.start() == CharOffset::ZERO {
            None
        } else {
            storage.char_at(offset)
        }
    });
    let after = storage.char_at(range.end());

    Ok(
        !before.is_some_and(|ch| config.word_boundary.is_identifier_continue(ch))
            && !after.is_some_and(|ch| config.word_boundary.is_identifier_continue(ch)),
    )
}

fn fold_text_with_char_boundaries(text: &str) -> FoldedText {
    let mut folded = String::new();
    let mut boundaries = vec![(0usize, 0usize)];

    for (char_offset, ch) in text.chars().enumerate() {
        folded.extend(ch.to_lowercase());
        boundaries.push((folded.len(), char_offset + 1));
    }

    FoldedText {
        text: folded,
        byte_boundaries_to_char_offsets: boundaries,
    }
}

impl FoldedText {
    fn char_offset_for_byte_boundary(&self, byte: usize) -> Option<usize> {
        self.byte_boundaries_to_char_offsets
            .binary_search_by_key(&byte, |(boundary, _)| *boundary)
            .ok()
            .map(|index| self.byte_boundaries_to_char_offsets[index].1)
    }
}

fn char_count_until(text: &str, byte: usize) -> usize {
    text[..byte].chars().count()
}

fn next_char_boundary(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len() + 1;
    }

    let mut next = byte + 1;
    while next < text.len() && !text.is_char_boundary(next) {
        next += 1;
    }
    next
}

fn text_range(start: CharOffset, end: CharOffset) -> TextRange {
    TextRange::new(start, end).expect("internal invariant: search start <= end")
}
