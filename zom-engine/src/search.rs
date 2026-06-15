//! 当前 Buffer / Snapshot 内的 literal search 与 regex search 契约。
//!
//! 本模块只实现单 Buffer 文本搜索结果模型、纯字符串匹配和正则匹配；不做跨文件索引，也不承担 UI 高亮语义。

use crate::{
    BufferConfig, BufferVersion, ByteOffset, CoordinateError, EngineError, EngineResult,
    SearchError, Stickiness, TextRange, VersionedResult, position_map::MappingResult,
    search_async::SearchControl, storage::TextRead, transaction::DeltaEvent,
};
use regex::{Regex, RegexBuilder};
use regex_automata::meta;

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
///
/// 不再带 haystack 字节预算——搜索已经异步、可取消、有进度上报（见[`crate::SearchHandle`]），调用方靠 cancel 控制大文件搜索的退出，不靠引擎在物化阶段提前拒绝。
/// 如果担心 regex 自身计算爆掉，仍可通过 `size_limit` / `dfa_size_limit` 控制 regex crate 内部的资源上限。
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

pub(crate) fn search_in_text_with_control<T: TextRead>(
    storage: &T,
    version: BufferVersion,
    config: &BufferConfig,
    query: &str,
    options: SearchOptions,
    control: &SearchControl,
) -> EngineResult<SearchResult> {
    if query.is_empty() {
        return Err(SearchError::EmptyQuery.into());
    }

    let search_range = resolve_search_range(storage, options.range())?;
    validate_search_range(storage, search_range)?;

    let matches = if options.is_case_sensitive() {
        find_case_sensitive_matches_streaming(
            storage,
            config,
            search_range,
            query,
            options,
            control,
        )?
    } else {
        // 大小写不敏感：流式 chunks 扫描 + 滑动折叠窗口，**不物化整个 haystack**。
        find_case_insensitive_matches_streaming(
            storage,
            config,
            search_range,
            query,
            options,
            control,
        )?
    };

    // 算法走完一次完整扫描——把进度补到末端，调用方读到的最后一份 progress 是 100%。
    control.finish_scan();

    Ok(SearchResult::new(
        version,
        query.to_string(),
        options,
        matches,
    ))
}

/// 流式 regex 搜索：按 rope chunks 逐步累积到有界窗口，永不一次性物化整个 haystack。
///
/// 滑动窗口 + 重叠区保证跨窗口边界匹配不被截断；
/// 若匹配命中 buffer 末端且还有更多数据则动态扩展——在罕见长匹配场景退化为物化，保证正确性。
fn search_regex_streaming<T: TextRead>(
    storage: &T,
    version: BufferVersion,
    pattern: &str,
    options: RegexSearchOptions,
    control: &SearchControl,
) -> EngineResult<RegexSearchResult> {
    let regex = build_regex_automata(pattern, options)?;
    let search_range = resolve_search_range(storage, options.range())?;
    validate_search_range(storage, search_range)?;

    let base_offset = search_range.start().get();

    const WINDOW_SIZE: usize = 256 * 1024; // 256 KiB
    const OVERLAP_MIN: usize = 4096;

    // 重叠区至少 pattern.len() * 8 或 4 KiB，不超过窗口一半
    let overlap = (pattern.len() * 8).clamp(OVERLAP_MIN, WINDOW_SIZE / 2);

    let mut buf: Vec<u8> = Vec::with_capacity(WINDOW_SIZE + overlap);
    // buf[0] 在文件中的绝对字节偏移
    let mut buf_start: usize = base_offset;
    let mut last_reported_end: usize = base_offset;
    let mut matches: Vec<SearchMatch> = Vec::new();
    let mut ordinal: usize = 0;
    let mut first_window = true;

    let mut chunks = storage.chunks(search_range)?;
    let mut chunks_exhausted = false;

    // 直到没有更多数据且 buffer 已处理完
    loop {
        // 向 buffer 填数据
        while buf.len() < WINDOW_SIZE {
            match chunks.next() {
                Some(chunk) => {
                    control.check_cancel()?;
                    buf.extend_from_slice(chunk.as_bytes());
                }
                None => {
                    chunks_exhausted = true;
                    break;
                }
            }
        }

        if buf.is_empty() {
            break;
        }

        // 搜索当前 buffer，若命中末端且还有更多数据则扩展 buffer 重搜。
        // 用于处理极长匹配（rare case），正常匹配不会触发。
        loop {
            let mut boundary_hit = false;
            for m in regex.find_iter(&buf[..]) {
                if m.end() == buf.len() && !chunks_exhausted {
                    boundary_hit = true;
                    break;
                }
            }

            if !boundary_hit {
                break;
            }

            // 扩展 buffer 并重搜
            match chunks.next() {
                Some(chunk) => buf.extend_from_slice(chunk.as_bytes()),
                None => {
                    chunks_exhausted = true;
                    break;
                }
            }
        }

        let new_data_boundary = if first_window { 0 } else { overlap };

        // 收割结果
        for m in regex.find_iter(&buf[..]) {
            let abs_end = buf_start + m.end();
            // 去重：跳过已上报的命中
            if abs_end <= last_reported_end {
                continue;
            }
            // 跳过完全落在重叠区的命中（已被前序窗口上报）
            if !first_window && m.end() <= new_data_boundary {
                continue;
            }

            let abs_start = buf_start + m.start();
            matches.push(SearchMatch::new(
                ordinal,
                text_range(ByteOffset::new(abs_start), ByteOffset::new(abs_end))?,
            ));
            ordinal += 1;
            last_reported_end = abs_end;
            // 进度按"已扫描到的 haystack 字节数"上报，单调推进
            control.set_scanned((last_reported_end - base_offset) as u64);
        }

        if chunks_exhausted {
            break;
        }

        // 剪裁 buffer：保留末尾 overlap 字节作为下一窗口的前缀
        let trim = buf.len().saturating_sub(overlap);
        if trim == 0 {
            // buffer 比 overlap 还小，继续累积
            continue;
        }
        buf_start += trim;
        buf.drain(..trim);
        first_window = false;
    }

    control.finish_scan();

    Ok(RegexSearchResult::new(
        version,
        pattern.to_string(),
        options,
        matches,
    ))
}

pub(crate) fn search_regex_in_text_with_control<T: TextRead>(
    storage: &T,
    version: BufferVersion,
    pattern: &str,
    options: RegexSearchOptions,
    control: &SearchControl,
) -> EngineResult<RegexSearchResult> {
    search_regex_streaming(storage, version, pattern, options, control)
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
    let haystack = regex_haystack_owned(storage, search_range)?;

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

fn build_regex_automata(pattern: &str, options: RegexSearchOptions) -> EngineResult<meta::Regex> {
    let syntax = regex_automata::util::syntax::Config::new()
        .case_insensitive(!options.is_case_sensitive())
        .multi_line(options.is_multi_line())
        .dot_matches_new_line(options.dot_matches_new_line());

    let meta_config =
        regex_automata::meta::Config::new().dfa_size_limit(Some(options.dfa_size_limit()));

    meta::Builder::new()
        .configure(meta_config)
        .syntax(syntax)
        .build(pattern)
        .map_err(|error| {
            SearchError::InvalidRegex {
                pattern: pattern.to_string(),
                message: error.to_string(),
            }
            .into()
        })
}

fn regex_haystack_owned<T: TextRead>(storage: &T, search_range: TextRange) -> EngineResult<String> {
    storage.slice_to_string(search_range)
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
    control: &SearchControl,
) -> EngineResult<Vec<SearchMatch>> {
    let mut matches = Vec::new();
    let mut carry = String::new();
    let mut scratch = String::new();
    let mut chunk_start = search_range.start().get();
    let mut next_allowed_start = chunk_start;
    let carry_limit = query.len().saturating_sub(1);

    for chunk in storage.chunks(search_range)? {
        // 每个 chunk 起点检查一次取消——ropey chunk ~ 4 KiB，最坏延迟在毫秒级。
        control.check_cancel()?;
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
        control.advance_scanned(chunk.len() as u64);
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
fn find_case_insensitive_matches_streaming<T: TextRead>(
    storage: &T,
    config: &BufferConfig,
    search_range: TextRange,
    query: &str,
    options: SearchOptions,
    control: &SearchControl,
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
        // 与 case-sensitive 路径对称：chunk 起点检查一次取消位。
        control.check_cancel()?;
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
        control.advance_scanned(chunk.len() as u64);
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
