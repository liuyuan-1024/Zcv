//! 异步搜索的句柄、取消位与进度上报。
//!
//! 本模块只承担"线程模型 + 控制通道"，匹配算法仍在 [`crate::search`]。两边以
//! [`SearchControl`] 单向通信：
//!
//! - 调用方持有 [`SearchHandle`]：可读 [`SearchProgress`]、可调 `cancel()`，也可阻塞 `join()` 或非阻塞 `try_join()` 取结果。
//! - 内核算法持有 [`SearchControl`]：在协作点调 `check_cancel()` / `advance_scanned()`。
//!
//! 取消是协作式的：内核必须主动调 `check_cancel`，没有抢占。当前注入点：
//! literal 流式扫描的每个 chunk、regex `find_iter` 的每次迭代。粒度 ≈ ropey chunk
//! (~4 KiB) 或一次正则前进，最坏延迟在毫秒级。
//!
//! Handle 被 drop 时自动 `cancel`，工作线程下次检查点即退出——不阻塞调用方。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use crate::{
    BufferConfig, BufferVersion, EngineResult, RegexSearchOptions, RegexSearchResult, SearchError,
    SearchOptions, SearchResult,
    search::{search_in_text_with_control, search_regex_in_text_with_control},
    storage::{RopeySnapshot, TextRead},
};

/// 异步搜索进度快照。调用方读取后可自行决定是否再次轮询。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchProgress {
    /// 已扫描的字节数；流式 literal 搜索按 chunk 推进，regex 搜索按命中前进。
    pub scanned_bytes: u64,
    /// 本次搜索覆盖的总字节数（spawn 时就已确定，等于 `search_range.len()`）。
    pub total_bytes: u64,
    /// 是否已被请求取消（已经调过 `cancel` 或 handle 已 drop）。
    pub cancelled: bool,
    /// 工作线程是否已退出（结果可立刻 `try_join` 取走）。
    pub finished: bool,
}

impl SearchProgress {
    /// `[0.0, 1.0]` 范围内的归一化进度；`total_bytes == 0` 时返回 1.0。
    pub fn ratio(self) -> f32 {
        if self.total_bytes == 0 {
            return 1.0;
        }
        (self.scanned_bytes as f32 / self.total_bytes as f32).clamp(0.0, 1.0)
    }
}

/// 工作线程与调用方共享的原子状态。私有：所有访问都通过 [`SearchHandle`] /
/// [`SearchControl`] 包装，避免被误用。
#[derive(Debug)]
struct Shared {
    cancel: AtomicBool,
    finished: AtomicBool,
    scanned: AtomicU64,
    total: u64,
}

impl Shared {
    fn new(total: u64) -> Arc<Self> {
        Arc::new(Self {
            cancel: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            scanned: AtomicU64::new(0),
            total,
        })
    }
}

/// 内核算法侧的协作把手——只暴露"检查取消"和"推进进度"两件事。
///
/// 该类型是 `pub(crate)`，仅 [`crate::search`] 模块使用；外部调用方拿到的是
/// [`SearchHandle`]。
#[derive(Debug, Clone)]
pub(crate) struct SearchControl {
    shared: Arc<Shared>,
}

impl SearchControl {
    /// 在循环关键点调用：若已被请求取消，返回 `SearchError::Cancelled`。
    ///
    /// `Relaxed` 已足够——取消只是控制位，命中后下一次检查必然能看到。
    pub(crate) fn check_cancel(&self) -> EngineResult<()> {
        if self.shared.cancel.load(Ordering::Relaxed) {
            return Err(SearchError::Cancelled.into());
        }
        Ok(())
    }

    /// 把已扫过的字节数推进 `delta`。仅用于进度上报，调用方失败容忍——多线程
    /// 偶发的可见性延迟不影响算法正确性。
    pub(crate) fn advance_scanned(&self, delta: u64) {
        if delta == 0 {
            return;
        }
        self.shared.scanned.fetch_add(delta, Ordering::Relaxed);
    }

    /// 把已扫过字节数直接置为 `value`（regex 路径用：拿到完整 haystack 后按
    /// match end 推进，单调递增）。
    pub(crate) fn set_scanned(&self, value: u64) {
        self.shared.scanned.store(value, Ordering::Relaxed);
    }

    /// 在 `total` 范围内把进度抬到末端，用于"扫描完成"的最后一次同步上报。
    pub(crate) fn finish_scan(&self) {
        self.shared
            .scanned
            .store(self.shared.total, Ordering::Relaxed);
    }
}

/// 一次异步搜索的句柄。
///
/// - 取消：`cancel()` 设置原子位；工作线程在下一次检查点退出。
/// - 进度：`progress()` 读当前 [`SearchProgress`] 快照，可重复读。
/// - 结果：`try_join()` 非阻塞取，`join()` 阻塞取，**两者只能调一次**。
///
/// Drop 自动 cancel；不会 join 工作线程，因此 handle 释放是非阻塞的。
#[derive(Debug)]
#[must_use = "SearchHandle 被 drop 即取消搜索；如需结果请调用 join() 或 try_join()"]
pub struct SearchHandle<T> {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<EngineResult<T>>>,
}

impl<T: Send + 'static> SearchHandle<T> {
    /// 请求取消正在运行的搜索。幂等——多次调用等价于一次。
    ///
    /// 返回后工作线程**可能**还在跑下一段不可分割的循环体；要等真正退出请用
    /// `is_finished()` 轮询或调用 `join()`。
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }

    /// 是否已被请求取消（不代表线程已经退出）。
    pub fn is_cancelled(&self) -> bool {
        self.shared.cancel.load(Ordering::Relaxed)
    }

    /// 工作线程是否已经退出。`true` 后 `try_join()` 必然立刻拿到结果。
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::Acquire)
    }

    /// 读取当前进度快照。
    pub fn progress(&self) -> SearchProgress {
        SearchProgress {
            scanned_bytes: self.shared.scanned.load(Ordering::Relaxed),
            total_bytes: self.shared.total,
            cancelled: self.is_cancelled(),
            finished: self.is_finished(),
        }
    }

    /// 非阻塞取结果。线程未结束时返回 `None`，结束后只能取到 `Some` 一次。
    ///
    /// # Panics
    /// 若已经调用过 `try_join()` 拿到 `Some` 或调用过 `join()`，再次调用将 panic。
    pub fn try_join(&mut self) -> Option<EngineResult<T>> {
        if !self.is_finished() {
            return None;
        }
        let handle = self
            .thread
            .take()
            .expect("SearchHandle::try_join 在结果已经取走后被再次调用");
        Some(join_thread(handle))
    }

    /// 阻塞等待线程退出并取走结果。
    ///
    /// # Panics
    /// 若已经 `try_join()` 拿过结果，再次 `join()` 将 panic。
    pub fn join(mut self) -> EngineResult<T> {
        let handle = self
            .thread
            .take()
            .expect("SearchHandle::join 在结果已经取走后被再次调用");
        join_thread(handle)
    }
}

impl<T> Drop for SearchHandle<T> {
    fn drop(&mut self) {
        // 自动 cancel：让工作线程在下一次检查点退出。
        // 不显式 join——thread 在结束前会清理自己；调用方 drop handle 应当非阻塞。
        self.shared.cancel.store(true, Ordering::Relaxed);
    }
}

fn join_thread<T>(handle: JoinHandle<EngineResult<T>>) -> EngineResult<T> {
    match handle.join() {
        Ok(result) => result,
        Err(_) => Err(crate::EngineError::EngineBug {
            location: "search_async::join_thread",
            detail: "搜索工作线程 panic".to_string(),
        }),
    }
}

/// 启动一次 literal 搜索：克隆 snapshot 进后台线程，立即返回 handle。
pub(crate) fn spawn_literal_search(
    snapshot: RopeySnapshot,
    version: BufferVersion,
    config: BufferConfig,
    query: String,
    options: SearchOptions,
) -> SearchHandle<SearchResult> {
    let total = effective_total_bytes(&snapshot, options.range());
    spawn(total, move |control| {
        search_in_text_with_control(&snapshot, version, &config, &query, options, control)
    })
}

/// 启动一次 regex 搜索：克隆 snapshot 进后台线程，立即返回 handle。
pub(crate) fn spawn_regex_search(
    snapshot: RopeySnapshot,
    version: BufferVersion,
    pattern: String,
    options: RegexSearchOptions,
) -> SearchHandle<RegexSearchResult> {
    let total = effective_total_bytes(&snapshot, options.range());
    spawn(total, move |control| {
        search_regex_in_text_with_control(&snapshot, version, &pattern, options, control)
    })
}

fn effective_total_bytes(snapshot: &RopeySnapshot, requested: Option<crate::TextRange>) -> u64 {
    match requested {
        Some(range) => range.len() as u64,
        None => snapshot.len_bytes().get() as u64,
    }
}

fn spawn<T, F>(total: u64, work: F) -> SearchHandle<T>
where
    T: Send + 'static,
    F: FnOnce(&SearchControl) -> EngineResult<T> + Send + 'static,
{
    let shared = Shared::new(total);
    let shared_for_thread = Arc::clone(&shared);
    let thread = thread::Builder::new()
        .name("zom-search".to_string())
        .spawn(move || {
            let control = SearchControl {
                shared: Arc::clone(&shared_for_thread),
            };
            let result = work(&control);
            // 即使被 cancel，也把 finished 抬起来——这是"线程已退出"的语义。
            shared_for_thread.finished.store(true, Ordering::Release);
            result
        })
        .expect("spawn zom-search 线程失败");

    SearchHandle {
        shared,
        thread: Some(thread),
    }
}
