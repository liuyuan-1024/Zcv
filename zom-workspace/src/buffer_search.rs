//! 单缓冲区搜索状态：挂在 `WorkspaceBuffer` 上，按缓冲区共享。
//!
//! 本模块承担：
//!
//! - 持有当前 query / options / 一次搜索的 `SearchResult` 或 `RegexSearchResult`
//! - 当前命中在结果集中的位置（用户点 next / prev 推进的指针）
//! - **编辑期间走 `try_remap` 增量推进**——不主动发现新命中
//! - query / options 变化、或结果落版本时由调用方调 `sync(buffer)` 同步重跑
//!
//! 同一缓冲区的多视图（分屏）共用一份 `BufferSearch`；跨缓冲区搜索是
//! 另一层服务（项目级搜索），不在此处。
//!
//! ## 增量推进的责任链
//!
//! ```text
//! 用户编辑缓冲区
//!     ↓
//! Buffer 内累积 DeltaEvent 到 pending_delta_events
//!     ↓
//! 调用方（命令派发后 / 编辑器命令落地后）调 WorkspaceBuffer::pump_post_edit()
//!     ↓
//! pump_post_edit 取走 pending events 逐个喂给 BufferSearch::apply_delta
//!     ↓
//! 已存命中按 try_remap 推进到新版本；被删 / 塌缩的整条丢弃
//! ```
//!
//! 不在 `sync(&Buffer)` 里隐式排空事件——`sync` 只接受 `&Buffer` 不可变借用，
//! 把事件排空与搜索读取拆开。每条 DeltaEvent 有唯一消费方（BufferSearch），
//! 避免多消费者并行 drain 同一队列。

use zom_engine::{
    Buffer, BufferVersion, DeltaEvent, EngineResult, RegexSearchOptions as EngineRegexOptions,
    RegexSearchResult, SearchOptions as EngineLiteralOptions, SearchResult, TextRange,
};

/// `BufferSearch` 暴露给调用方的可调选项。
///
/// 与 engine 内部的 `SearchOptions` / `RegexSearchOptions` 区分：本类型是
/// 「用户在 panel 上看到的复选框」的直接映射，包含 regex 开关；engine 那两个类型
/// 各自只承载 literal 或 regex 一种语义，由本模块按 `regex` 字段分发。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BufferSearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

/// 一次搜索结果——按是 literal 还是 regex 分两个变体。
///
/// 两边都带 `try_remap`，对外暴露统一的 ranges / hit count 接口。
#[derive(Clone, Debug, Eq, PartialEq)]
enum SearchSlot {
    Literal(SearchResult),
    Regex(RegexSearchResult),
}

impl SearchSlot {
    fn version(&self) -> BufferVersion {
        match self {
            Self::Literal(result) => result.version(),
            Self::Regex(result) => result.version(),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Literal(result) => result.len(),
            Self::Regex(result) => result.len(),
        }
    }

    fn range_at(&self, ordinal: usize) -> Option<TextRange> {
        match self {
            Self::Literal(result) => result.match_at(ordinal).map(|m| m.range()),
            Self::Regex(result) => result.match_at(ordinal).map(|m| m.range()),
        }
    }

    /// 借用底层 ranges 用来给阶段 2 / panel 渲染。返回 boxed iterator 来抹平
    /// 两个变体返回类型不同的差异——调用频次低（每帧一次、命中数 O(可见行)），
    /// 装箱开销可忽略。
    fn ranges(&self) -> Box<dyn Iterator<Item = TextRange> + '_> {
        match self {
            Self::Literal(result) => Box::new(result.ranges()),
            Self::Regex(result) => Box::new(result.ranges()),
        }
    }

    fn try_remap(self, event: &DeltaEvent) -> EngineResult<Self> {
        Ok(match self {
            Self::Literal(result) => Self::Literal(result.try_remap(event)?),
            Self::Regex(result) => Self::Regex(result.try_remap(event)?),
        })
    }
}

/// 单缓冲区的搜索状态。
///
/// 默认构造（`BufferSearch::default()`）是"空白"：query 空、options 全 false、
/// 无结果、无当前命中。调用方通过 `set_query` / `set_options` 喂入用户输入，
/// 然后调 `sync(buffer)` 在下一个机会运行 engine 搜索。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BufferSearch {
    query: String,
    options: BufferSearchOptions,
    slot: Option<SearchSlot>,
    /// 当前命中在 `slot.matches()` 中的下标（0-based）。`None` 表示空结果集
    /// 或还没跑过。导航命令把它推进 / 倒退；replace 后由 `pump_delta` 经过
    /// try_remap 自然减一或失效。
    current_hit: Option<usize>,
}

impl BufferSearch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn options(&self) -> BufferSearchOptions {
        self.options
    }

    /// 写入新的 query。**不立刻跑搜索**——调用方下次调 `sync` 时统一触发。
    ///
    /// 返回 `true` 表示文本确有变化（用以让 panel 决定是否 `cx.notify()` 之类）。
    pub fn set_query(&mut self, query: String) -> bool {
        if self.query == query {
            return false;
        }
        self.query = query;
        // query 变化意味着旧结果必然作废；丢掉避免下次 sync 误判可用而漏重跑。
        self.slot = None;
        self.current_hit = None;
        true
    }

    /// 写入新的 options。同样不立刻跑搜索；任何字段变化都视作「旧结果作废」。
    pub fn set_options(&mut self, options: BufferSearchOptions) -> bool {
        if self.options == options {
            return false;
        }
        self.options = options;
        self.slot = None;
        self.current_hit = None;
        true
    }

    /// 当前结果集中的命中数；尚未跑过 / query 空 / 结果集空时返回 0。
    pub fn hit_count(&self) -> usize {
        self.slot.as_ref().map(|s| s.len()).unwrap_or(0)
    }

    /// 当前命中的 1-based 序号；无当前命中时为 `None`。直接给 panel
    /// 的「3 / 27」标签消费。
    pub fn current_hit_ordinal(&self) -> Option<usize> {
        self.current_hit.map(|index| index + 1)
    }

    /// 当前命中对应的 TextRange，给 reveal / selection 用。
    pub fn current_range(&self) -> Option<TextRange> {
        let slot = self.slot.as_ref()?;
        let index = self.current_hit?;
        slot.range_at(index)
    }

    /// 当前结果的版本；无结果时 `None`。
    pub fn result_version(&self) -> Option<BufferVersion> {
        self.slot.as_ref().map(|s| s.version())
    }

    /// 遍历所有命中的 TextRange，按 ordinal 升序。给 EditorView 阶段 2 用。
    pub fn ranges(&self) -> Box<dyn Iterator<Item = TextRange> + '_> {
        match &self.slot {
            Some(slot) => slot.ranges(),
            None => Box::new(std::iter::empty()),
        }
    }

    /// 让 BufferSearch 与缓冲区当前版本对齐：query 非空且**没有可用结果**时
    /// 重新调 engine 跑搜索；"可用"指 slot 存在且版本与 buffer 一致。
    ///
    /// 编辑期间的版本推进由 [`Self::apply_delta`] 通过 try_remap 维护；只有
    /// remap 失败（被删 / 塌缩的命中）造成 slot 主动重置、或 query/options 变化
    /// 才走本函数的重跑分支。**`sync` 不主动 drain pending DeltaEvent**——那是
    /// [`crate::WorkspaceBuffer::pump_post_edit`] 的责任，避免多消费者并行 drain。
    pub fn sync(&mut self, buffer: &Buffer) -> EngineResult<()> {
        if self.query.is_empty() {
            // 没有 query 就清空状态——避免 query 从有变无时残留旧结果。
            self.slot = None;
            self.current_hit = None;
            return Ok(());
        }

        let buffer_version = buffer.version();
        let needs_rerun = match &self.slot {
            None => true,
            // 结果在过去某次 apply_delta 被 try_remap 推进过；只要版本对得上就保留。
            Some(slot) => slot.version() != buffer_version,
        };

        if needs_rerun {
            self.slot = Some(self.run_search(buffer)?);
            self.normalize_current_hit_after_rerun();
        }
        Ok(())
    }

    /// 把一条 DeltaEvent 喂进来：现存命中按 try_remap 推进到新版本。
    ///
    /// `event.old_version()` 必须与当前 `slot.version()` 一致——这是
    /// `WorkspaceBuffer::pump_post_edit` 顺序投递的天然保证。如果调用方跳跃版本
    /// （例如漏掉中间事件），engine 的 try_remap 会原子拒绝并返回错误，BufferSearch
    /// 把 slot 丢掉退化为「无结果」，下次 sync 自然重跑。
    ///
    /// 没有 slot 时无操作：还没搜索过，不需要 remap。
    pub fn apply_delta(&mut self, event: &DeltaEvent) -> EngineResult<()> {
        let Some(slot) = self.slot.take() else {
            return Ok(());
        };
        match slot.try_remap(event) {
            Ok(new_slot) => {
                self.slot = Some(new_slot);
                self.clamp_current_hit_into_slot();
            }
            Err(_) => {
                // remap 失败：版本错位或其他原子拒绝。丢掉结果，下次 sync 重跑。
                self.current_hit = None;
            }
        }
        Ok(())
    }

    /// 推进当前命中到下一个命中（环绕）。空结果集无操作。返回推进后命中的
    /// 范围以便上层做 reveal / selection。
    ///
    /// 调用方在调本方法**之前**应该已经调过 `sync(buffer)`，使结果与 query 一致。
    pub fn advance(&mut self) -> Option<TextRange> {
        let slot = self.slot.as_ref()?;
        let count = slot.len();
        if count == 0 {
            return None;
        }
        let next = match self.current_hit {
            Some(index) => (index + 1) % count,
            None => 0,
        };
        self.current_hit = Some(next);
        slot.range_at(next)
    }

    /// 倒退当前命中到上一个命中（环绕）。
    pub fn retreat(&mut self) -> Option<TextRange> {
        let slot = self.slot.as_ref()?;
        let count = slot.len();
        if count == 0 {
            return None;
        }
        let prev = match self.current_hit {
            Some(0) | None => count - 1,
            Some(index) => index - 1,
        };
        self.current_hit = Some(prev);
        slot.range_at(prev)
    }

    /// 给 replace 命令读：当前 result 与对应 ordinal。返回 `None` 表示「没东西可
    /// 替换」——空结果集 / 无当前命中。
    ///
    /// 让调用方拿着这两个去调 `buffer.replace_search_match` / `replace_regex_match`，
    /// 替换成功后必须紧跟一次 `WorkspaceBuffer::pump_post_edit` 把新生的 DeltaEvent 喂回来。
    /// 本模块不内嵌替换处理器——边界留给 zom-command。
    pub fn current_for_replace(&self) -> Option<CurrentReplaceTarget<'_>> {
        let slot = self.slot.as_ref()?;
        let ordinal = self.current_hit?;
        Some(CurrentReplaceTarget { slot, ordinal })
    }

    /// 给 replace_all 命令读：当前完整 result。
    pub fn result_for_replace(&self) -> Option<CurrentReplaceTarget<'_>> {
        let slot = self.slot.as_ref()?;
        Some(CurrentReplaceTarget { slot, ordinal: 0 })
    }

    fn run_search(&self, buffer: &Buffer) -> EngineResult<SearchSlot> {
        if self.options.regex {
            let pattern = regex_pattern(&self.query, self.options.whole_word);
            let options =
                EngineRegexOptions::new().with_case_sensitive(self.options.case_sensitive);
            buffer
                .search_regex(&pattern, options)
                .map(SearchSlot::Regex)
        } else {
            let options = EngineLiteralOptions::new()
                .with_case_sensitive(self.options.case_sensitive)
                .with_whole_word(self.options.whole_word);
            buffer.search(&self.query, options).map(SearchSlot::Literal)
        }
    }

    /// 跑完搜索后挑一个合理的当前命中：
    /// - 旧 current_hit 在新结果集内仍合法 → 保持
    /// - 否则若结果集非空 → 跳到第 0 项（与 panel「搜索完落到第一条」习惯一致）
    /// - 空结果集 → `None`
    fn normalize_current_hit_after_rerun(&mut self) {
        let count = self.slot.as_ref().map(|s| s.len()).unwrap_or(0);
        self.current_hit = match self.current_hit {
            Some(index) if index < count => Some(index),
            _ if count > 0 => Some(0),
            _ => None,
        };
    }

    /// remap 后命中数可能减少（被删的整条丢）；把 current_hit 夹回合法范围。
    /// 这里**不强制**把 current_hit 推到 0——保留原位置让「替换当前命中」之类的
    /// 节奏自然衔接（被替换那条会被 remap 吃掉，下一条自动顶上来到原 index）。
    fn clamp_current_hit_into_slot(&mut self) {
        let count = self.slot.as_ref().map(|s| s.len()).unwrap_or(0);
        self.current_hit = match self.current_hit {
            Some(index) if index < count => Some(index),
            _ if count > 0 => Some(count - 1),
            _ => None,
        };
    }
}

/// 给 zom-command 的替换处理器用：拿这个去调 buffer 的 replace API。
///
/// 故意只暴露访问器、不可 Clone——让调用方在持有 BufferSearch 不可变借用的
/// 同时无法逃逸；replace 完毕调 `WorkspaceBuffer::pump_post_edit` 是契约。
pub struct CurrentReplaceTarget<'a> {
    slot: &'a SearchSlot,
    ordinal: usize,
}

impl<'a> CurrentReplaceTarget<'a> {
    pub fn ordinal(&self) -> usize {
        self.ordinal
    }

    pub fn literal(&self) -> Option<&'a SearchResult> {
        match self.slot {
            SearchSlot::Literal(result) => Some(result),
            SearchSlot::Regex(_) => None,
        }
    }

    pub fn regex(&self) -> Option<&'a RegexSearchResult> {
        match self.slot {
            SearchSlot::Regex(result) => Some(result),
            SearchSlot::Literal(_) => None,
        }
    }
}

/// 整词选项打开时，把 regex 模式两端套上 `\b`；关闭时原样返回。
///
/// 用 `\b(?:{query})\b` 而不是 `\b{query}\b`：query 内部如果有 `|` 之类，外层
/// 的 `\b` 只会绑到第一段 / 最后一段——加非捕获组兜住整个 alternation 才是
/// 一致的整词语义。
fn regex_pattern(query: &str, whole_word: bool) -> String {
    if whole_word {
        format!(r"\b(?:{query})\b")
    } else {
        query.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use zom_engine::{BufferConfig, BufferOrigin};

    fn make_buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    fn make_file_buffer(text: &str) -> Buffer {
        Buffer::from_reader(
            BufferOrigin::external("test"),
            Cursor::new(text.as_bytes().to_vec()),
            BufferConfig::default(),
        )
        .unwrap()
    }

    /// 空 query → sync 后无结果、ranges 为空。
    #[test]
    fn empty_query_sync_should_leave_no_results() {
        let buffer = make_buffer("hello world");
        let mut search = BufferSearch::new();
        search.sync(&buffer).unwrap();

        assert_eq!(search.hit_count(), 0);
        assert!(search.current_hit_ordinal().is_none());
        assert!(search.ranges().next().is_none());
    }

    /// 写入 query 并 sync 后能拿到全部命中；当前命中默认落在第一条。
    #[test]
    fn sync_should_run_search_and_anchor_first_hit() {
        let buffer = make_buffer("foo bar foo baz foo");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        search.sync(&buffer).unwrap();

        assert_eq!(search.hit_count(), 3);
        assert_eq!(search.current_hit_ordinal(), Some(1));
        let ranges: Vec<TextRange> = search.ranges().collect();
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0].start().get(), 0);
    }

    /// 同样的 query 再 sync 不重跑。靠两次 sync 之间 buffer 版本不变来确认。
    #[test]
    fn sync_should_skip_when_query_and_version_unchanged() {
        let buffer = make_buffer("foo foo");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        search.sync(&buffer).unwrap();
        let version_after_first = search.result_version().unwrap();

        search.sync(&buffer).unwrap();
        assert_eq!(search.result_version(), Some(version_after_first));
    }

    /// query 变化触发下一次 sync 重跑。
    #[test]
    fn changing_query_should_invalidate_results_until_resync() {
        let buffer = make_buffer("foo bar");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        search.sync(&buffer).unwrap();
        assert_eq!(search.hit_count(), 1);

        search.set_query("bar".to_string());
        assert_eq!(search.hit_count(), 0); // 设新 query 时清掉旧 slot
        search.sync(&buffer).unwrap();
        assert_eq!(search.hit_count(), 1);
        // 验证新结果指向 "bar" 而非旧的 "foo"
        let first = search.ranges().next().unwrap();
        assert_eq!(first.start().get(), 4);
    }

    /// options 变化同样视作旧结果作废。
    #[test]
    fn changing_options_should_invalidate_results_until_resync() {
        let buffer = make_buffer("Foo foo FOO");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        search.set_options(BufferSearchOptions {
            case_sensitive: false,
            ..BufferSearchOptions::default()
        });
        search.sync(&buffer).unwrap();
        assert_eq!(search.hit_count(), 3);

        search.set_options(BufferSearchOptions {
            case_sensitive: true,
            ..BufferSearchOptions::default()
        });
        search.sync(&buffer).unwrap();
        assert_eq!(search.hit_count(), 1);
    }

    /// advance / retreat 在结果集上环绕。
    #[test]
    fn advance_and_retreat_should_wrap_around_hits() {
        let buffer = make_buffer("foo foo foo");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        search.sync(&buffer).unwrap();
        assert_eq!(search.current_hit_ordinal(), Some(1));

        search.advance();
        assert_eq!(search.current_hit_ordinal(), Some(2));
        search.advance();
        assert_eq!(search.current_hit_ordinal(), Some(3));
        search.advance(); // 向前回卷
        assert_eq!(search.current_hit_ordinal(), Some(1));

        search.retreat(); // 向后回卷
        assert_eq!(search.current_hit_ordinal(), Some(3));
    }

    /// regex 模式 + whole_word 走 `\b(?:...)\b` 包装。
    #[test]
    fn regex_with_whole_word_should_match_only_full_words() {
        let buffer = make_buffer("foo foobar");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        search.set_options(BufferSearchOptions {
            regex: true,
            whole_word: true,
            ..BufferSearchOptions::default()
        });
        search.sync(&buffer).unwrap();

        assert_eq!(search.hit_count(), 1);
        let only = search.ranges().next().unwrap();
        assert_eq!(only.start().get(), 0);
        assert_eq!(only.end().get(), 3);
    }

    /// 编辑后没调 apply_delta：sync 自身检测到落版本，重跑搜索。
    #[test]
    fn sync_should_rerun_when_buffer_version_advanced_without_apply_delta() {
        let mut buffer = make_file_buffer("foo foo");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        search.sync(&buffer).unwrap();
        assert_eq!(search.hit_count(), 2);

        // 插入一段不改变命中数的文本，buffer.version 推进
        let original_version = buffer.version();
        buffer.insert(zom_engine::ByteOffset::new(0), "x ").unwrap();
        assert_ne!(buffer.version(), original_version);

        // 不喂 DeltaEvent：sync 检测版本落后，自然重跑
        search.sync(&buffer).unwrap();
        assert_eq!(search.hit_count(), 2);
        assert_eq!(search.result_version(), Some(buffer.version()));
    }

    /// 编辑后调 apply_delta：现存命中按 try_remap 推进，无需重跑。
    #[test]
    fn apply_delta_should_remap_existing_hits_without_rerun() {
        let mut buffer = make_file_buffer("foo bar foo");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        search.sync(&buffer).unwrap();
        let version_before = search.result_version().unwrap();
        assert_eq!(search.hit_count(), 2);

        // 在两个 foo 之间插入文字——第二个 foo 的 range 会被 remap 后移
        buffer
            .insert(zom_engine::ByteOffset::new(4), "XX ")
            .unwrap();
        let event = buffer.take_pending_events().pop().unwrap();
        search.apply_delta(&event).unwrap();

        // remap 成功：命中数不变，版本跟上 buffer
        assert_eq!(search.hit_count(), 2);
        assert_eq!(search.result_version(), Some(buffer.version()));
        assert_ne!(search.result_version(), Some(version_before));

        // 验证第二个 foo 的 range 确实向后挪了 3 字节（"XX "）
        let second = search.ranges().nth(1).unwrap();
        assert_eq!(second.start().get(), 11); // 原来 8，+3
    }
}
