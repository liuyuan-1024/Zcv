//! 当前 Buffer / Snapshot 内的 literal search 与 regex search 契约。
//!
//! 本模块只实现单 Buffer 文本搜索结果模型、纯字符串匹配和正则匹配；不做跨文件索引，
//! 也不承担 UI 高亮语义。

use crate::{
    BufferConfig, BufferVersion, ByteOffset, CoordinateError, EngineError, EngineResult,
    MetadataLayer, MetadataLayerKind, MetadataRangeSpec, SearchError, Stickiness, TextRange,
    VersionedResult, position_map::MappingResult, storage::TextRead,
    tracking::TrackedRangeUpdatePolicy, transaction::DeltaEvent,
};
use regex::{Regex, RegexBuilder};

const DEFAULT_REGEX_HAYSTACK_BYTE_LIMIT: usize = 8 * 1024 * 1024;

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
    haystack_byte_limit: usize,
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
            haystack_byte_limit: DEFAULT_REGEX_HAYSTACK_BYTE_LIMIT,
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

    /// 设置 regex 搜索允许连续物化的最大 haystack 字节数。
    ///
    /// 当前 regex crate API 仍要求一段连续 haystack；这里把这条高成本路径显式预算化。
    /// `0` 表示调用方明确接受无限制物化。
    pub const fn with_haystack_byte_limit(mut self, haystack_byte_limit: usize) -> Self {
        self.haystack_byte_limit = haystack_byte_limit;
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

    pub const fn haystack_byte_limit(self) -> usize {
        self.haystack_byte_limit
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
    const fn new(ordinal: usize, range: TextRange) -> Self {
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
///
/// 版本绑定与过期判断由内部 `VersionedResult<Vec<SearchMatch>>` 承担；本类型本身只保留
/// query / options 等业务输入，避免与 `VersionedResult` 重复维护版本守卫语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    matches: VersionedResult<Vec<SearchMatch>>,
    query: String,
    options: SearchOptions,
}

impl SearchResult {
    fn new(
        version: BufferVersion,
        query: String,
        options: SearchOptions,
        matches: Vec<SearchMatch>,
    ) -> Self {
        Self {
            matches: VersionedResult::new(version, matches),
            query,
            options,
        }
    }

    pub fn version(&self) -> BufferVersion {
        self.matches.version()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn options(&self) -> SearchOptions {
        self.options
    }

    pub fn matches(&self) -> &[SearchMatch] {
        self.matches.value()
    }

    pub fn match_at(&self, ordinal: usize) -> Option<SearchMatch> {
        self.matches.value().get(ordinal).copied()
    }

    pub fn ranges(&self) -> impl Iterator<Item = TextRange> + '_ {
        self.matches
            .value()
            .iter()
            .map(|search_match| search_match.range())
    }

    pub fn len(&self) -> usize {
        self.matches.value().len()
    }

    pub fn is_empty(&self) -> bool {
        self.matches.value().is_empty()
    }

    pub fn is_stale(&self, current_version: BufferVersion) -> bool {
        self.matches.is_stale(current_version)
    }

    /// 已过期时丢弃，未过期时保留。
    pub fn discard_if_stale(self, current_version: BufferVersion) -> Option<Self> {
        if self.is_stale(current_version) {
            None
        } else {
            Some(self)
        }
    }

    /// 通过一次 `DeltaEvent` 把搜索结果推进到新版本。
    ///
    /// `event.old_version()` 必须与当前结果版本一致，否则原子拒绝；
    /// 命中映射 `Mapped` 的匹配按新坐标保留并连续重排 ordinal，
    /// `Deleted` / `Collapsed` 的匹配（被删除或塌缩为零宽）整条丢弃。
    /// query / options 由调用方自行决定是否需要在新版本上重新搜索。
    pub fn try_remap(self, event: &DeltaEvent) -> EngineResult<Self> {
        let Self {
            matches,
            query,
            options,
        } = self;
        let matches = matches.try_remap(event, |old_matches, position_map| {
            Ok(remap_search_matches(old_matches, position_map))
        })?;
        Ok(Self {
            matches,
            query,
            options,
        })
    }

    /// 把搜索结果转换为可跟随文本变化的 SearchMatch metadata layer。
    pub fn to_metadata_layer(&self) -> EngineResult<MetadataLayer<SearchMatchMetadata>> {
        build_search_match_metadata_layer(self.matches.version(), self.matches.value(), &self.query)
    }
}

/// 一次正则搜索结果，绑定被搜索文本的 `BufferVersion`。
///
/// 版本绑定与过期判断由内部 `VersionedResult<Vec<SearchMatch>>` 承担；本类型本身只保留
/// pattern / options 等业务输入，避免与 `VersionedResult` 重复维护版本守卫语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexSearchResult {
    matches: VersionedResult<Vec<SearchMatch>>,
    pattern: String,
    options: RegexSearchOptions,
}

impl RegexSearchResult {
    fn new(
        version: BufferVersion,
        pattern: String,
        options: RegexSearchOptions,
        matches: Vec<SearchMatch>,
    ) -> Self {
        Self {
            matches: VersionedResult::new(version, matches),
            pattern,
            options,
        }
    }

    pub fn version(&self) -> BufferVersion {
        self.matches.version()
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn options(&self) -> RegexSearchOptions {
        self.options
    }

    pub fn matches(&self) -> &[SearchMatch] {
        self.matches.value()
    }

    pub fn match_at(&self, ordinal: usize) -> Option<SearchMatch> {
        self.matches.value().get(ordinal).copied()
    }

    pub fn ranges(&self) -> impl Iterator<Item = TextRange> + '_ {
        self.matches
            .value()
            .iter()
            .map(|search_match| search_match.range())
    }

    pub fn len(&self) -> usize {
        self.matches.value().len()
    }

    pub fn is_empty(&self) -> bool {
        self.matches.value().is_empty()
    }

    pub fn is_stale(&self, current_version: BufferVersion) -> bool {
        self.matches.is_stale(current_version)
    }

    /// 已过期时丢弃，未过期时保留。
    pub fn discard_if_stale(self, current_version: BufferVersion) -> Option<Self> {
        if self.is_stale(current_version) {
            None
        } else {
            Some(self)
        }
    }

    /// 通过一次 `DeltaEvent` 把正则搜索结果推进到新版本。
    ///
    /// `event.old_version()` 必须与当前结果版本一致，否则原子拒绝；
    /// 命中映射 `Mapped` 的匹配按新坐标保留并连续重排 ordinal，
    /// `Deleted` / `Collapsed` 的匹配整条丢弃。pattern / options 由调用方
    /// 自行决定是否需要在新版本上重新搜索；regex 替换 / capture 展开必须基于
    /// 同版本上的新结果，不应基于 remap 后的结果。
    pub fn try_remap(self, event: &DeltaEvent) -> EngineResult<Self> {
        let Self {
            matches,
            pattern,
            options,
        } = self;
        let matches = matches.try_remap(event, |old_matches, position_map| {
            Ok(remap_search_matches(old_matches, position_map))
        })?;
        Ok(Self {
            matches,
            pattern,
            options,
        })
    }

    /// 把正则搜索结果转换为可跟随文本变化的 SearchMatch metadata layer。
    pub fn to_metadata_layer(&self) -> EngineResult<MetadataLayer<SearchMatchMetadata>> {
        build_search_match_metadata_layer(
            self.matches.version(),
            self.matches.value(),
            &self.pattern,
        )
    }
}

/// 把搜索匹配按 `PositionMap::map_old_range_with_stickiness(Stickiness::Never)`
/// 推进到新坐标；只保留 `Mapped` 的匹配，`Deleted` / `Collapsed` / `Ambiguous`
/// 一律丢弃。保留下来的匹配按出现顺序连续重排 ordinal 从 0 开始，与原始
/// 搜索结果构造时的 ordinal 约定保持一致。
fn remap_search_matches(
    matches: Vec<SearchMatch>,
    position_map: &crate::position_map::PositionMap,
) -> Vec<SearchMatch> {
    let mut remapped = Vec::with_capacity(matches.len());
    for search_match in matches {
        if let MappingResult::Mapped(new_range) =
            position_map.map_old_range_with_stickiness(search_match.range(), Stickiness::Never)
        {
            remapped.push(SearchMatch::new(remapped.len(), new_range));
        }
    }
    remapped
}

fn build_search_match_metadata_layer(
    version: BufferVersion,
    matches: &[SearchMatch],
    query: &str,
) -> EngineResult<MetadataLayer<SearchMatchMetadata>> {
    let ranges = matches.iter().map(|search_match| {
        MetadataRangeSpec::new(
            search_match.range(),
            SearchMatchMetadata::new(search_match.ordinal(), query.to_string()),
        )
        .with_stickiness(Stickiness::Never)
        .with_update_policy(TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion())
    });

    let mut layer = MetadataLayer::with_kind(MetadataLayerKind::SearchMatch, version);
    layer.replace_all_with_options(version, ranges)?;
    Ok(layer)
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

    let search_range = resolve_search_range(storage, options.range())?;
    validate_search_range(storage, search_range)?;

    let matches = if options.is_case_sensitive() {
        find_case_sensitive_matches_streaming(storage, config, search_range, query, options)?
    } else {
        // 大小写不敏感：流式 chunks 扫描 + 滑动折叠窗口，**不物化整个 haystack**。
        find_case_insensitive_matches_streaming(storage, config, search_range, query, options)?
    };

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
    let search_range = resolve_search_range(storage, options.range())?;
    validate_search_range(storage, search_range)?;

    let base_offset = search_range.start().get();
    let haystack = regex_haystack(storage, search_range, options)?;
    let mut matches = Vec::new();
    for (ordinal, regex_match) in regex.find_iter(haystack.as_ref()).enumerate() {
        // regex 是 byte-native：直接拼上 base_offset
        let start = base_offset + regex_match.start();
        let end = base_offset + regex_match.end();
        matches.push(SearchMatch::new(
            ordinal,
            text_range(ByteOffset::new(start), ByteOffset::new(end))?,
        ));
    }

    Ok(RegexSearchResult::new(
        version,
        pattern.to_string(),
        options,
        matches,
    ))
}

pub(crate) fn regex_replacements_in_text<'a, T: TextRead>(
    storage: &T,
    result: &RegexSearchResult,
    replacement: &'a str,
) -> EngineResult<impl Iterator<Item = EngineResult<(TextRange, String)>> + 'a> {
    let regex = build_regex(result.pattern(), result.options())?;
    let search_range = resolve_search_range(storage, result.options().range())?;
    validate_search_range(storage, search_range)?;

    let base_offset = search_range.start().get();
    let haystack = regex_haystack_owned(storage, search_range, result.options())?;

    Ok(RegexReplacementIter {
        regex,
        haystack,
        base_offset,
        replacement,
        next_start: 0,
        done: false,
    })
}

pub(crate) fn regex_replacement_for_match<T: TextRead>(
    storage: &T,
    result: &RegexSearchResult,
    ordinal: usize,
    replacement: &str,
) -> EngineResult<Option<(TextRange, String)>> {
    for (index, regex_replacement) in
        regex_replacements_in_text(storage, result, replacement)?.enumerate()
    {
        let replacement = regex_replacement?;
        if index == ordinal {
            return Ok(Some(replacement));
        }
    }

    Ok(None)
}

struct RegexReplacementIter<'a> {
    regex: Regex,
    haystack: String,
    base_offset: usize,
    replacement: &'a str,
    next_start: usize,
    done: bool,
}

impl Iterator for RegexReplacementIter<'_> {
    type Item = EngineResult<(TextRange, String)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.next_start > self.haystack.len() {
            return None;
        }

        let captures = self.regex.captures_at(&self.haystack, self.next_start)?;
        let Some(regex_match) = captures.get(0) else {
            self.done = true;
            return None;
        };

        let start = regex_match.start();
        let end = regex_match.end();
        self.next_start = next_regex_search_start(&self.haystack, start, end);
        if self.next_start > self.haystack.len() {
            self.done = true;
        }

        let mut expanded = String::new();
        captures.expand(self.replacement, &mut expanded);
        Some(
            text_range(
                ByteOffset::new(self.base_offset + start),
                ByteOffset::new(self.base_offset + end),
            )
            .map(|range| (range, expanded)),
        )
    }
}

/// 统一解析 `SearchOptions::range()`：未指定时默认全文。
fn resolve_search_range<T: TextRead>(
    storage: &T,
    requested: Option<TextRange>,
) -> EngineResult<TextRange> {
    match requested {
        Some(r) => Ok(r),
        None => text_range(ByteOffset::ZERO, storage.len_bytes()),
    }
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

fn regex_haystack<'a, T: TextRead>(
    storage: &'a T,
    search_range: TextRange,
    options: RegexSearchOptions,
) -> EngineResult<std::borrow::Cow<'a, str>> {
    validate_regex_haystack_budget(search_range, options)?;
    storage.slice_text(search_range)
}

fn regex_haystack_owned<T: TextRead>(
    storage: &T,
    search_range: TextRange,
    options: RegexSearchOptions,
) -> EngineResult<String> {
    validate_regex_haystack_budget(search_range, options)?;
    storage.slice_to_string(search_range)
}

fn validate_regex_haystack_budget(
    search_range: TextRange,
    options: RegexSearchOptions,
) -> EngineResult<()> {
    let limit = options.haystack_byte_limit();
    if limit != 0 && search_range.len() > limit {
        return Err(SearchError::RangeTooLarge {
            range_bytes: search_range.len(),
            limit,
        }
        .into());
    }

    Ok(())
}

fn validate_search_range<T: TextRead>(storage: &T, range: TextRange) -> EngineResult<()> {
    if range.end() > storage.len_bytes() {
        return Err(CoordinateError::OutOfBounds(range.end()).into());
    }

    Ok(())
}

fn find_case_sensitive_matches_streaming<T: TextRead>(
    storage: &T,
    config: &BufferConfig,
    search_range: TextRange,
    query: &str,
    options: SearchOptions,
) -> EngineResult<Vec<SearchMatch>> {
    let mut matches = Vec::new();
    let mut carry = String::new();
    let mut scratch = String::new();
    let mut chunk_start = search_range.start().get();
    let mut next_allowed_start = chunk_start;
    let carry_limit = query.len().saturating_sub(1);

    for chunk in storage.chunks(search_range)? {
        let (scan_base, scan) = if carry.is_empty() {
            (chunk_start, chunk)
        } else {
            scratch.clear();
            scratch.push_str(&carry);
            scratch.push_str(chunk);
            (chunk_start - carry.len(), scratch.as_str())
        };

        let mut search_from = 0usize;
        while search_from <= scan.len() {
            let Some(relative_byte_start) = scan[search_from..].find(query) else {
                break;
            };
            let byte_start = search_from + relative_byte_start;
            let byte_end = byte_start + query.len();
            let absolute_start = scan_base + byte_start;
            let absolute_end = scan_base + byte_end;

            if absolute_end <= chunk_start || absolute_start < next_allowed_start {
                search_from = next_char_boundary_after(scan, byte_start);
                continue;
            }

            let range = text_range(
                ByteOffset::new(absolute_start),
                ByteOffset::new(absolute_end),
            )?;

            if accepts_match(storage, config, range, options)? {
                matches.push(SearchMatch::new(matches.len(), range));
                next_allowed_start = absolute_end;
                search_from = byte_end;
            } else {
                search_from = next_char_boundary_after(scan, byte_start);
            }
        }

        carry_suffix(&mut carry, scan, carry_limit);
        chunk_start += chunk.len();
    }

    Ok(matches)
}

/// **流式 case-insensitive 搜索**：按 chunks 走，永不物化整个 haystack；
/// 滑动折叠窗口仅 ~ `query.len()` 的常数倍内存。
///
/// 算法：
/// 1. 折叠查询串一次（`query.chars().flat_map(char::to_lowercase)`）
/// 2. 逐 chunk + 逐 char 推进，把每个字符的折叠结果追加到滑动 byte 窗口
/// 3. 每个折叠 byte 同步记录"它来自原文哪个 byte 起点"
/// 4. 窗口尾巴等于折叠查询时即匹配；非重叠：清空窗口继续
/// 5. 窗口超过 `q_len * 8` 时批量裁剪到 `q_len * 2`（amortized O(N) total）
///
/// 这彻底消除了旧实现 `fold_text_with_byte_boundaries` 的 **O(char_count) 边界表 +
/// 全文折叠 String** 双重 O(N) 内存占用——对 1GB 文本曾经会塞 ≥ 2-3GB 内存，
/// 现在内存与 query 长度同阶。
fn find_case_insensitive_matches_streaming<T: TextRead>(
    storage: &T,
    config: &BufferConfig,
    search_range: TextRange,
    query: &str,
    options: SearchOptions,
) -> EngineResult<Vec<SearchMatch>> {
    let folded_query: String = query.chars().flat_map(char::to_lowercase).collect();
    if folded_query.is_empty() {
        return Ok(Vec::new());
    }
    let folded_query_bytes = folded_query.as_bytes();
    let q_len = folded_query_bytes.len();

    let base_offset = search_range.start().get();

    // 滑动窗口：折叠 byte + 每个 byte 对应的"在 search_range 内的原文 byte 起点"
    let mut folded_buf: Vec<u8> = Vec::with_capacity(q_len * 4);
    let mut origs: Vec<usize> = Vec::with_capacity(q_len * 4);
    let mut matches: Vec<SearchMatch> = Vec::new();

    // 跨 chunk 时累计已遍历的 byte 数，用来推算每个 char 在 search_range 内部的 byte 偏移。
    let mut bytes_consumed_in_range = 0usize;
    let mut encode_buf = [0u8; 4];

    for chunk in storage.chunks(search_range)? {
        for (offset_in_chunk, ch) in chunk.char_indices() {
            let orig_byte_in_range = bytes_consumed_in_range + offset_in_chunk;
            let orig_byte_end_in_range = orig_byte_in_range + ch.len_utf8();

            // 折叠当前字符并把每个输出 byte 入栈（共享同一个 orig_byte 起点）
            for folded_ch in ch.to_lowercase() {
                let folded_str = folded_ch.encode_utf8(&mut encode_buf);
                for &b in folded_str.as_bytes() {
                    folded_buf.push(b);
                    origs.push(orig_byte_in_range);
                }
            }

            // 尾部匹配检查
            if folded_buf.len() >= q_len {
                let tail_start = folded_buf.len() - q_len;
                if &folded_buf[tail_start..] == folded_query_bytes {
                    let match_orig_start = base_offset + origs[tail_start];
                    let match_orig_end = base_offset + orig_byte_end_in_range;
                    let range = text_range(
                        ByteOffset::new(match_orig_start),
                        ByteOffset::new(match_orig_end),
                    )?;

                    if accepts_match(storage, config, range, options)? {
                        matches.push(SearchMatch::new(matches.len(), range));
                        // 非重叠：清空窗口，从下一字符开始重新累积
                        folded_buf.clear();
                        origs.clear();
                    }
                }
            }

            // 周期性裁剪窗口，保持有界内存
            if folded_buf.len() > q_len * 8 {
                let to_drop = folded_buf.len() - q_len * 2;
                folded_buf.drain(..to_drop);
                origs.drain(..to_drop);
            }
        }
        bytes_consumed_in_range += chunk.len();
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

    let before = if range.start() == ByteOffset::ZERO {
        None
    } else {
        // 找前一个 grapheme/字符起点，安全读取
        let prev = storage.previous_grapheme_boundary(range.start())?;
        storage.char_at_byte(prev)
    };
    let after = storage.char_at_byte(range.end());

    Ok(
        !before.is_some_and(|ch| config.word_boundary.is_identifier_continue(ch))
            && !after.is_some_and(|ch| config.word_boundary.is_identifier_continue(ch)),
    )
}

/// 内部 byte 区间构造：调用方应保证 `start <= end`；违反时返回 `EngineBug`，**永不 panic**。
///
/// 这取代了 Phase 1.4 残留的 `expect("内部不变量：search start <= end")`，
/// 让 search.rs 路径不再有不安全的 panic 出口。
fn text_range(start: ByteOffset, end: ByteOffset) -> EngineResult<TextRange> {
    TextRange::new(start, end).map_err(|_| EngineError::EngineBug {
        location: "search::text_range",
        detail: format!("生成了反向区间：start（{start:?}）> end（{end:?}）"),
    })
}

fn carry_suffix(carry: &mut String, text: &str, max_bytes: usize) {
    carry.clear();
    if max_bytes == 0 || text.is_empty() {
        return;
    }

    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    carry.push_str(&text[start..]);
}

fn next_char_boundary_after(text: &str, offset: usize) -> usize {
    let mut next = offset.saturating_add(1);
    while next < text.len() && !text.is_char_boundary(next) {
        next += 1;
    }
    next
}

fn next_regex_search_start(haystack: &str, start: usize, end: usize) -> usize {
    if end > start {
        return end;
    }

    if end == haystack.len() {
        return haystack.len() + 1;
    }

    next_char_boundary_after(haystack, end)
}
