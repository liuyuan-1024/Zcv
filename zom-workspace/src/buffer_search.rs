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
    RegexSearchResult, SearchHandle, SearchOptions as EngineLiteralOptions, SearchResult, TextRange,
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

/// 一次正在跑的后台搜索。`sync` 在 spawn 时存入，poll 在 handle 完成时取走。
///
/// Drop 即自动 `cancel`——`set_query` / `set_options` / `apply_delta` 都靠
/// 直接 `take` 来取消。
#[derive(Debug)]
enum PendingSearch {
    Literal(SearchHandle<SearchResult>),
    Regex(SearchHandle<RegexSearchResult>),
}

impl PendingSearch {
    fn is_finished(&self) -> bool {
        match self {
            Self::Literal(h) => h.is_finished(),
            Self::Regex(h) => h.is_finished(),
        }
    }
}

/// 一次 `sync` 调用对状态机的推进结果，调用方据此决定是否要 reveal / repaint。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchSyncOutcome {
    /// 空 query，或结果已与 buffer 版本一致；没有 in-flight。
    Idle,
    /// 后台搜索仍在跑——`slot` 可能为空（首次）或保留上一份未与 query 一致的结果。
    Pending,
    /// 本帧有一份新 slot 刚落地（结果与 buffer 当前版本对齐）。调用方可借机
    /// reveal 首条命中、要求重绘。
    JustReady,
    /// 后台结果完成但版本已落后（apply_delta 在期间推进过 buffer），已丢弃；
    /// 下次 `sync` 会重新 spawn。
    StaleDiscarded,
}

/// `install_finished` 内部决策结果——只在本文件内流通，外部看 `SearchSyncOutcome`。
enum Installed {
    Fresh,
    Stale,
    Failed,
}

/// 单缓冲区的搜索状态。
///
/// 默认构造（`BufferSearch::default()`）是"空白"：query 空、options 全 false、
/// 无结果、无当前命中。调用方通过 `set_query` / `set_options` 喂入用户输入，
/// 然后调 `sync(buffer)` 在下一个机会推进状态机。
///
/// ## 异步契约
///
/// `sync` **不阻塞**：query 非空且结果不一致时投出一个后台搜索（`pending`），
/// 立即返回 `Pending`。下次 `sync`（或独立调 `pump_pending` / 渲染线程任意
/// poll 钩子）发现 handle 已完成，便把结果落入 `slot` 并返回 `JustReady`。
/// 设计取舍：
///
/// - **打字期间清空旧 slot**（`set_query` / `set_options` 触发）：用户看到
///   "命中数空 → 重新跳"，避免上个 query 的高亮误导。
/// - **编辑期间立刻取消 pending**（`apply_delta`）：buffer 推进后过期结果必然
///   被丢弃，与其等它跑完不如尽早释放 worker 给下一拍 spawn。
#[derive(Debug, Default)]
pub struct BufferSearch {
    query: String,
    options: BufferSearchOptions,
    slot: Option<SearchSlot>,
    /// 当前命中在 `slot.matches()` 中的下标（0-based）。`None` 表示空结果集
    /// 或还没跑过。导航命令把它推进 / 倒退；replace 后由 `pump_delta` 经过
    /// try_remap 自然减一或失效。
    current_hit: Option<usize>,
    /// 正在跑的后台搜索；`sync` spawn 进来，下次轮询取走。
    /// 不参与 PartialEq——`SearchHandle` 不可比较，外部测试看 `is_searching()`。
    pending: Option<PendingSearch>,
}

impl Clone for BufferSearch {
    /// 克隆出来的副本不带 in-flight pending——`SearchHandle` 不可 clone，且
    /// 让副本继承未完成的搜索没有可定义的语义（多消费者抢一个结果）。
    /// 实际使用中 BufferSearch 不应被频繁 clone；clone 主要是测试 / 调试便利。
    fn clone(&self) -> Self {
        Self {
            query: self.query.clone(),
            options: self.options,
            slot: self.slot.clone(),
            current_hit: self.current_hit,
            pending: None,
        }
    }
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
    ///
    /// 副作用：旧 slot 和 in-flight pending 都被清掉，调用方下一次读 `ranges()` /
    /// `hit_count()` 即得空，新 query 的结果由后续 sync 落地。
    pub fn set_query(&mut self, query: String) -> bool {
        if self.query == query {
            return false;
        }
        self.query = query;
        self.invalidate();
        true
    }

    /// 写入新的 options。同样不立刻跑搜索；任何字段变化都视作「旧结果作废」。
    pub fn set_options(&mut self, options: BufferSearchOptions) -> bool {
        if self.options == options {
            return false;
        }
        self.options = options;
        self.invalidate();
        true
    }

    /// 当前是否有后台搜索在跑。给上层判断是否需要再调一次 poll / 显示进度指示。
    pub fn is_searching(&self) -> bool {
        self.pending.is_some()
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

    /// 把状态机推一拍。**非阻塞**：先收割可能已完成的后台搜索，再判断是否
    /// 需要 spawn 新的——本身不等结果。
    ///
    /// 返回值：
    /// - `Idle`：query 空，或 slot 已与 buffer 当前版本一致且没有 in-flight；
    /// - `Pending`：有后台搜索在跑（可能本帧刚 spawn，也可能继承自上次）；
    /// - `JustReady`：本帧收割到一份新 slot 落地；
    /// - `StaleDiscarded`：收割到的结果版本已过期（apply_delta 在期间推进过
    ///   buffer），已丢；同帧立即 spawn 新 pending，调用方会再看到 `Pending`
    ///   或下次 `JustReady`。
    ///
    /// 调用规则与原同步版本一致：放在 `WorkspaceBuffer::pump_post_edit` 之后，
    /// 让 try_remap 先把已有 slot 推进到新版本，再决定要不要 re-spawn。
    pub fn sync(&mut self, buffer: &Buffer) -> EngineResult<SearchSyncOutcome> {
        if self.query.is_empty() {
            // 没有 query 就清空状态——避免 query 从有变无时残留旧结果。
            self.invalidate();
            return Ok(SearchSyncOutcome::Idle);
        }

        let mut outcome = self.pump_pending(buffer);

        let buffer_version = buffer.version();
        let needs_spawn = self.pending.is_none()
            && match &self.slot {
                None => true,
                // try_remap 推进过 slot；只要版本对得上就不需要重跑。
                Some(slot) => slot.version() != buffer_version,
            };

        if needs_spawn {
            self.pending = Some(self.spawn_search(buffer));
            // 若本帧没收割到新结果（最常见情况），把状态从 Idle 升级到 Pending。
            if matches!(outcome, SearchSyncOutcome::Idle) {
                outcome = SearchSyncOutcome::Pending;
            }
        }

        Ok(outcome)
    }

    /// 单独把"收割已完成的后台搜索"剥出来，供渲染线程每帧 poll——不 spawn 新的。
    ///
    /// `sync` 内部也调本函数；分开是为了让"render-time pump"和"dispatch-time
    /// sync"两条触发点共用同一个收割路径。
    pub fn pump_pending(&mut self, buffer: &Buffer) -> SearchSyncOutcome {
        let Some(pending) = self.pending.as_ref() else {
            return SearchSyncOutcome::Idle;
        };
        if !pending.is_finished() {
            return SearchSyncOutcome::Pending;
        }
        let pending = self.pending.take().expect("just checked Some");
        match self.install_finished(buffer, pending) {
            Installed::Fresh => SearchSyncOutcome::JustReady,
            Installed::Stale => SearchSyncOutcome::StaleDiscarded,
            Installed::Failed => SearchSyncOutcome::StaleDiscarded,
        }
    }

    /// 阻塞驱动：测试 / 命令式场景下"等结果到手再返回"。
    ///
    /// 每轮先 `sync`，若仍有 pending 就阻塞 `join` 拿结果再装入 slot，循环直到
    /// 状态进入 `Idle` 或 `JustReady` 且没有再 spawn。生产代码不应调用——会卡
    /// 调用线程；用 `sync` + 渲染循环的 `pump_pending` 才是非阻塞路径。
    pub fn wait_until_idle(&mut self, buffer: &Buffer) -> EngineResult<()> {
        loop {
            self.sync(buffer)?;
            let Some(pending) = self.pending.take() else {
                return Ok(());
            };
            // 拿当前 pending 阻塞到完成，把结果灌回去（可能 fresh / stale，都通过
            // install_finished 走同一条版本判断）。
            let _ = self.install_finished(buffer, pending);
        }
    }

    /// 把 `set_query` / `set_options` / 空 query 时丢旧状态的逻辑收一处。
    /// pending 通过 `take` 触发 `Drop`，自动调 `SearchHandle::cancel`。
    fn invalidate(&mut self) {
        self.slot = None;
        self.current_hit = None;
        self.pending = None;
    }

    /// 把一条 DeltaEvent 喂进来：现存命中按 try_remap 推进到新版本，**同时取消
    /// 任何 in-flight pending**——pending 是基于旧版本 snapshot 跑的，结果落地时
    /// 版本对不上必然被丢弃，与其等不如尽早释放。下次 `sync` 会基于新版本重新
    /// spawn。
    ///
    /// `event.old_version()` 必须与当前 `slot.version()` 一致——这是
    /// `WorkspaceBuffer::pump_post_edit` 顺序投递的天然保证。如果调用方跳跃版本
    /// （例如漏掉中间事件），engine 的 try_remap 会原子拒绝并返回错误，BufferSearch
    /// 把 slot 丢掉退化为「无结果」，下次 sync 自然重跑。
    pub fn apply_delta(&mut self, event: &DeltaEvent) -> EngineResult<()> {
        // pending 必然过期：丢掉触发自动 cancel。
        self.pending = None;

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

    /// 启动一次后台搜索；立刻返回 handle，不等。
    fn spawn_search(&self, buffer: &Buffer) -> PendingSearch {
        if self.options.regex {
            let pattern = regex_pattern(&self.query, self.options.whole_word);
            let options =
                EngineRegexOptions::new().with_case_sensitive(self.options.case_sensitive);
            PendingSearch::Regex(buffer.search_regex(&pattern, options))
        } else {
            let options = EngineLiteralOptions::new()
                .with_case_sensitive(self.options.case_sensitive)
                .with_whole_word(self.options.whole_word);
            PendingSearch::Literal(buffer.search(&self.query, options))
        }
    }

    /// 已完成的 pending → slot；做版本 / 错误判断。调用前保证 `pending.is_finished()`。
    fn install_finished(&mut self, buffer: &Buffer, pending: PendingSearch) -> Installed {
        let buffer_version = buffer.version();
        let (version, install): (BufferVersion, Box<dyn FnOnce(&mut Self)>) = match pending {
            PendingSearch::Literal(handle) => match handle.join() {
                Ok(result) => (
                    result.version(),
                    Box::new(move |this| this.slot = Some(SearchSlot::Literal(result))),
                ),
                Err(_) => return Installed::Failed,
            },
            PendingSearch::Regex(handle) => match handle.join() {
                Ok(result) => (
                    result.version(),
                    Box::new(move |this| this.slot = Some(SearchSlot::Regex(result))),
                ),
                Err(_) => return Installed::Failed,
            },
        };
        if version != buffer_version {
            // apply_delta 已经在 spawn 之后清掉了 pending，理论上这里碰不到 version
            // 不一致；保留兜底——比如未来引入 cancel-on-edit 之外的路径。
            return Installed::Stale;
        }
        install(self);
        self.normalize_current_hit_after_rerun();
        Installed::Fresh
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

    /// 测试帮手：sync + 阻塞驱动到 pending 收割完。生产代码不要这么用——
    /// `sync` 自身非阻塞，结果在下一次 `sync` / `pump_pending` 时落地。
    fn sync_blocking(search: &mut BufferSearch, buffer: &Buffer) {
        search.wait_until_idle(buffer).unwrap();
    }

    /// 空 query → sync 后无结果、ranges 为空。
    #[test]
    fn empty_query_sync_should_leave_no_results() {
        let buffer = make_buffer("hello world");
        let mut search = BufferSearch::new();
        let outcome = search.sync(&buffer).unwrap();
        assert_eq!(outcome, SearchSyncOutcome::Idle);

        assert_eq!(search.hit_count(), 0);
        assert!(search.current_hit_ordinal().is_none());
        assert!(search.ranges().next().is_none());
        assert!(!search.is_searching());
    }

    /// 写入 query 并 sync 后能拿到全部命中；当前命中默认落在第一条。
    #[test]
    fn sync_should_run_search_and_anchor_first_hit() {
        let buffer = make_buffer("foo bar foo baz foo");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        sync_blocking(&mut search, &buffer);

        assert_eq!(search.hit_count(), 3);
        assert_eq!(search.current_hit_ordinal(), Some(1));
        let ranges: Vec<TextRange> = search.ranges().collect();
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0].start().get(), 0);
    }

    /// 同样的 query 再 sync 不重跑。靠两次 sync 之间 buffer 版本不变 +
    /// `Idle` outcome 来确认。
    #[test]
    fn sync_should_skip_when_query_and_version_unchanged() {
        let buffer = make_buffer("foo foo");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        sync_blocking(&mut search, &buffer);
        let version_after_first = search.result_version().unwrap();

        let outcome = search.sync(&buffer).unwrap();
        assert_eq!(outcome, SearchSyncOutcome::Idle);
        assert_eq!(search.result_version(), Some(version_after_first));
    }

    /// query 变化触发下一次 sync 重跑。
    #[test]
    fn changing_query_should_invalidate_results_until_resync() {
        let buffer = make_buffer("foo bar");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        sync_blocking(&mut search, &buffer);
        assert_eq!(search.hit_count(), 1);

        search.set_query("bar".to_string());
        assert_eq!(search.hit_count(), 0); // 设新 query 时清掉旧 slot
        sync_blocking(&mut search, &buffer);
        assert_eq!(search.hit_count(), 1);
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
        sync_blocking(&mut search, &buffer);
        assert_eq!(search.hit_count(), 3);

        search.set_options(BufferSearchOptions {
            case_sensitive: true,
            ..BufferSearchOptions::default()
        });
        sync_blocking(&mut search, &buffer);
        assert_eq!(search.hit_count(), 1);
    }

    /// advance / retreat 在结果集上环绕。
    #[test]
    fn advance_and_retreat_should_wrap_around_hits() {
        let buffer = make_buffer("foo foo foo");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        sync_blocking(&mut search, &buffer);
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
        sync_blocking(&mut search, &buffer);

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
        sync_blocking(&mut search, &buffer);
        assert_eq!(search.hit_count(), 2);

        let original_version = buffer.version();
        buffer.insert(zom_engine::ByteOffset::new(0), "x ").unwrap();
        assert_ne!(buffer.version(), original_version);

        // 不喂 DeltaEvent：sync 检测版本落后，spawn 新的；wait_until_idle 等结果。
        sync_blocking(&mut search, &buffer);
        assert_eq!(search.hit_count(), 2);
        assert_eq!(search.result_version(), Some(buffer.version()));
    }

    /// 编辑后调 apply_delta：现存命中按 try_remap 推进，无需重跑。
    #[test]
    fn apply_delta_should_remap_existing_hits_without_rerun() {
        let mut buffer = make_file_buffer("foo bar foo");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        sync_blocking(&mut search, &buffer);
        let version_before = search.result_version().unwrap();
        assert_eq!(search.hit_count(), 2);

        buffer
            .insert(zom_engine::ByteOffset::new(4), "XX ")
            .unwrap();
        let event = buffer.take_pending_events().pop().unwrap();
        search.apply_delta(&event).unwrap();

        // remap 成功：命中数不变，版本跟上 buffer
        assert_eq!(search.hit_count(), 2);
        assert_eq!(search.result_version(), Some(buffer.version()));
        assert_ne!(search.result_version(), Some(version_before));

        // 第二个 foo 的 range 向后挪了 3 字节（"XX "）
        let second = search.ranges().nth(1).unwrap();
        assert_eq!(second.start().get(), 11);
    }

    /// 异步：set_query 后 sync 返回 Pending，slot 未到位；pump_pending 收割。
    #[test]
    fn sync_should_return_pending_then_just_ready_across_two_calls() {
        let buffer = make_buffer("alpha beta alpha");
        let mut search = BufferSearch::new();
        search.set_query("alpha".to_string());

        let first = search.sync(&buffer).unwrap();
        assert_eq!(first, SearchSyncOutcome::Pending);
        assert!(search.is_searching());

        // 等到 handle 真的完成（生产由渲染线程每帧 pump）。
        while search.is_searching() {
            search.pump_pending(&buffer);
            std::thread::yield_now();
        }
        assert_eq!(search.hit_count(), 2);
    }

    /// apply_delta 必须取消 in-flight pending——下次 sync 会基于新版本 spawn。
    #[test]
    fn apply_delta_should_cancel_in_flight_pending_search() {
        let mut buffer = make_file_buffer("foo foo");
        let mut search = BufferSearch::new();
        search.set_query("foo".to_string());
        let outcome = search.sync(&buffer).unwrap();
        assert!(matches!(
            outcome,
            SearchSyncOutcome::Pending | SearchSyncOutcome::JustReady
        ));
        assert!(search.is_searching() || search.hit_count() == 2);

        buffer
            .insert(zom_engine::ByteOffset::new(0), "x ")
            .unwrap();
        let event = buffer.take_pending_events().pop().unwrap();
        search.apply_delta(&event).unwrap();
        // pending 被取消；slot 可能保留也可能没有（取决于 sync 先后），关键是
        // 这里不再有 in-flight 任务挂着。
        assert!(!search.is_searching());

        sync_blocking(&mut search, &buffer);
        assert_eq!(search.hit_count(), 2);
        assert_eq!(search.result_version(), Some(buffer.version()));
    }
}
