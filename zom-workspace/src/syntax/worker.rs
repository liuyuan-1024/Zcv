//! 后台 SyntaxWorker：一根 std::thread 串行承载所有 buffer 的 provider 调用。
//!
//! 设计依据：[改造方案 §3.2 / §3.3](../../docs/语法高亮异步增量改造.md)。
//!
//! ## 总体形态
//!
//! ```text
//!  main thread                       worker thread
//!  ───────────                       ─────────────
//!  BufferSyntaxState ──post Job──▶  recv → match { Attach | Edit | Detach }
//!                       (mpsc)             ↓
//!  pump_pending_highlights ◀── sink ──── provider.{attach,on_edit,detach}
//!         (drain → layers)                ↓
//!                                     HashMap<BufferId, Entry>
//! ```
//!
//! 关键边界：
//! - **Provider 实例的真实归属在 worker**：`Box<dyn HighlightProvider>` 通过
//!   `Job::Attach` 从主线程交接到 worker；worker 退出前以 `Detach` 回收并 drop。
//! - **Sink 是轻量 clone 的 Arc**：worker 推产物，主线程在
//!   [`crate::Workspace::pump_pending_highlights`] 中 drain。两侧解耦。
//! - **Job 投递不阻塞主线程**：`mpsc::Sender::send` 在标准 channel 上是 lock-free
//!   的入队；64 MiB rust 单键的主线程消耗只剩"克隆 Snapshot + ChangeSet + 推一次
//!   channel"。
//! - **Panic 守护**：每条 Job 用 `catch_unwind` 包住；某 buffer 的 provider 触发
//!   panic 不会拖垮线程，只把出问题的 entry 丢弃，buffer 退化成 plain；其他 buffer
//!   不受影响。
//! - **同步等待**：[`SyntaxWorkerHandle::wait_for_idle`] 仅给测试 / bench 使用；
//!   正常前台路径不该走，否则就吃掉异步收益。
//!
//! ## Edit 折叠
//!
//! 每轮 `recv` 唤醒后，worker 先用 `try_recv` 把 channel 里所有已就绪 Job 一次性
//! 抽干，再按 FIFO 处理。处理过程中如果连续若干 Job 都是同一 buffer 的 `Edit`，
//! 把前 N-1 条交给 [`HighlightProvider::apply_pending_edit`]（只推进 tree.edit、
//! 不重 parse / query / push），仅最后一条走 `on_edit` 做完整 reparse + query +
//! sink。这样**连续按键 N 次只产生一次 reparse 和一次 sink push**，避免中间
//! 产物被立刻覆盖的浪费。
//!
//! 不同 buffer 的 Edit 或非 Edit Job（Attach / Detach / SetViewport）会切断
//! 折叠，按原顺序逐条处理——避免越界打乱跨 buffer / 跨任务种类的语义。
//!
//! ## 不做的事
//!
//! - **不跨 buffer 重排 Job**：折叠只对**严格连续**的同 buffer Edit 生效；遇到
//!   其他类型或别的 buffer 立即收手。
//! - **不共享 Parser 跨 buffer**：每个 provider 仍自带 Parser；跨语言复用 parser 是内存优化项，与异步路径正交。

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use zom_engine::{BufferVersion, ChangeSet, Snapshot, TextRange};

use crate::BufferId;

use super::provider::{BufferHandle, HighlightProvider};
use super::sink::HighlightSink;

/// 主线程 → worker 的请求。
enum Job {
    Attach {
        buffer_id: BufferId,
        provider: Box<dyn HighlightProvider>,
        snapshot: Snapshot,
        sink: HighlightSink,
        /// 可选的初始 viewport——若 `Some(range)`，worker 在 `provider.attach`
        /// 之前先调一次 `set_viewport(Some(range))`，让 attach 内部的 `run_full`
        /// 立刻按 viewport 范围跑 query 并以 `ReplaceRange` 投递；不必等到第一次
        /// 滚动 / 编辑才补齐高亮。
        ///
        /// 调用方在 attach 时已知 viewport（典型场景：desktop 打开文件时已经选好
        /// 活动 view 与首屏 byte range）时传 `Some`；否则传 `None`，由后续
        /// `Job::SetViewport` 异步建立 hint。
        initial_viewport: Option<TextRange>,
    },
    Edit {
        buffer_id: BufferId,
        change: ChangeSet,
        snapshot: Snapshot,
        new_version: BufferVersion,
    },
    Detach {
        buffer_id: BufferId,
    },
    SetViewport {
        buffer_id: BufferId,
        byte_range: Option<TextRange>,
    },
}

struct Entry {
    provider: Box<dyn HighlightProvider>,
    sink: HighlightSink,
}

/// 在飞任务计数器 + Condvar，给 [`SyntaxWorkerHandle::wait_for_idle`] 用。
struct InFlight {
    count: AtomicUsize,
    mutex: Mutex<()>,
    cv: Condvar,
}

impl InFlight {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            count: AtomicUsize::new(0),
            mutex: Mutex::new(()),
            cv: Condvar::new(),
        })
    }

    fn inc(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    fn dec_and_notify(&self) {
        let prev = self.count.fetch_sub(1, Ordering::SeqCst);
        if prev == 1 {
            // 拿一下 mutex 与 wait 端配对，避免错过 notify。
            let _g = self.mutex.lock().expect("InFlight 互斥锁中毒");
            self.cv.notify_all();
        }
    }

    fn wait_idle(&self) {
        let mut guard = self.mutex.lock().expect("InFlight 互斥锁中毒");
        while self.count.load(Ordering::SeqCst) > 0 {
            guard = self.cv.wait(guard).expect("InFlight 条件变量中毒");
        }
    }
}

/// 后台 SyntaxWorker 的对外句柄。
///
/// `Arc<SyntaxWorkerHandle>` 在 [`crate::Workspace`] 与每个
/// [`crate::syntax::BufferSyntaxState`] 之间共享：所有 buffer 走同一根 worker
/// 线程串行。Drop 时关闭 channel 并 join 线程，避免后台还在跑就被回收。
pub struct SyntaxWorkerHandle {
    tx: Mutex<Option<Sender<Job>>>,
    in_flight: Arc<InFlight>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for SyntaxWorkerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntaxWorkerHandle")
            .field("in_flight", &self.in_flight.count.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl SyntaxWorkerHandle {
    /// 启动 worker 线程并返回主线程句柄。
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        let in_flight = InFlight::new();
        let in_flight_w = Arc::clone(&in_flight);
        let join = thread::Builder::new()
            .name("zom-syntax-worker".to_string())
            .spawn(move || worker_loop(rx, in_flight_w))
            .expect("必须能启动 zom-syntax-worker 线程");
        Self {
            tx: Mutex::new(Some(tx)),
            in_flight,
            join: Mutex::new(Some(join)),
        }
    }

    fn post(&self, job: Job) {
        self.in_flight.inc();
        let guard = self.tx.lock().expect("syntax worker 发送端互斥锁中毒");
        match guard.as_ref() {
            Some(tx) => {
                if tx.send(job).is_err() {
                    // 通道已断（worker panic 后线程退出之类），主线程不阻塞；
                    // 把刚加的 in_flight 回退掉。
                    self.in_flight.dec_and_notify();
                }
            }
            None => {
                // shutdown 中：丢弃。
                self.in_flight.dec_and_notify();
            }
        }
    }

    pub(crate) fn attach(
        &self,
        buffer_id: BufferId,
        provider: Box<dyn HighlightProvider>,
        snapshot: Snapshot,
        sink: HighlightSink,
        initial_viewport: Option<TextRange>,
    ) {
        self.post(Job::Attach {
            buffer_id,
            provider,
            snapshot,
            sink,
            initial_viewport,
        });
    }

    pub(crate) fn edit(
        &self,
        buffer_id: BufferId,
        change: ChangeSet,
        snapshot: Snapshot,
        new_version: BufferVersion,
    ) {
        self.post(Job::Edit {
            buffer_id,
            change,
            snapshot,
            new_version,
        });
    }

    pub(crate) fn detach(&self, buffer_id: BufferId) {
        self.post(Job::Detach { buffer_id });
    }

    pub(crate) fn set_viewport(&self, buffer_id: BufferId, byte_range: Option<TextRange>) {
        self.post(Job::SetViewport {
            buffer_id,
            byte_range,
        });
    }

    /// 阻塞直到当前已投递的所有任务都被 worker 处理完。
    ///
    /// **只给测试 / bench 用**。前台代码走每帧 [`crate::Workspace::pump_pending_highlights`]
    /// 即可——异步收益的前提就是主线程不在这里同步等。
    pub fn wait_for_idle(&self) {
        self.in_flight.wait_idle();
    }

    /// 把 JoinHandle 抢出来 detach。drop 时不再 join——主线程不需要等 worker
    /// 把积压任务跑完就能退出。
    ///
    /// **只给 bench / 长跑测试用**。bench 里我们要测「主线程投 1000 个任务
    /// 的耗时」，但工艺上 worker 真处理 1000 个 64 MiB 的重解析可能跑几十分钟；
    /// 测完直接 detach 让进程退出。前台代码不能调本方法——会让 worker 漏处理
    /// 末尾 Detach / 偏后 Edit，影响其他 buffer 的 layer 正确性。
    pub fn forget_join(&self) {
        if let Ok(mut guard) = self.join.lock() {
            if let Some(join) = guard.take() {
                drop(join); // JoinHandle::drop 是 detach，线程继续跑直到自然退出。
            }
        }
    }
}

impl Drop for SyntaxWorkerHandle {
    fn drop(&mut self) {
        // 先丢 sender 让 recv 返回 Err 触发 worker 退出循环；
        // 再 join 等线程实际结束，避免后台还在跑就被回收。
        if let Ok(mut guard) = self.tx.lock() {
            guard.take();
        }
        if let Some(join) = self.join.lock().ok().and_then(|mut g| g.take()) {
            let _ = join.join();
        }
    }
}

fn worker_loop(rx: Receiver<Job>, in_flight: Arc<InFlight>) {
    let mut entries: HashMap<BufferId, Entry> = HashMap::new();
    while let Ok(first) = rx.recv() {
        // 抽干 channel：把当前已排队的 Job 一次性取出再按 FIFO 处理。
        // 折叠路径（同 buffer 连续 Edit 合并）只在这种"已知队尾"的局部视图内做。
        let mut pending: VecDeque<Job> = VecDeque::new();
        pending.push_back(first);
        while let Ok(next) = rx.try_recv() {
            pending.push_back(next);
        }

        while let Some(job) = pending.pop_front() {
            match job {
                Job::Edit {
                    buffer_id,
                    change,
                    snapshot,
                    new_version,
                } => {
                    // 折叠同 buffer 的后续连续 Edit。`pending.front()` 是按 send 顺序
                    // 推入队首，因此一旦匹配上就 pop_front 拿到所有权。
                    let mut batch: Vec<(ChangeSet, Snapshot, BufferVersion)> =
                        vec![(change, snapshot, new_version)];
                    while matches!(
                        pending.front(),
                        Some(Job::Edit { buffer_id: bid, .. }) if *bid == buffer_id,
                    ) {
                        let Some(Job::Edit {
                            change: c,
                            snapshot: s,
                            new_version: v,
                            ..
                        }) = pending.pop_front()
                        else {
                            unreachable!("matches! 已保证此处必为同 buffer 的 Edit")
                        };
                        batch.push((c, s, v));
                    }
                    let batched = batch.len();
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        process_edit_batch(buffer_id, batch, &mut entries);
                    }));
                    if let Err(payload) = result {
                        log_panic(payload);
                    }
                    for _ in 0..batched {
                        in_flight.dec_and_notify();
                    }
                }
                other => {
                    let result = catch_unwind(AssertUnwindSafe(|| process(other, &mut entries)));
                    if let Err(payload) = result {
                        log_panic(payload);
                    }
                    in_flight.dec_and_notify();
                }
            }
        }
    }
}

/// 处理同一 buffer 上一段连续 Edit：前 N-1 条只 advance 内部状态，最后一条做完整
/// reparse + query + sink push。`batch` 必须非空，按 FIFO 排列。
fn process_edit_batch(
    buffer_id: BufferId,
    batch: Vec<(ChangeSet, Snapshot, BufferVersion)>,
    entries: &mut HashMap<BufferId, Entry>,
) {
    debug_assert!(!batch.is_empty(), "Edit batch 不能为空");
    let Some(entry) = entries.get_mut(&buffer_id) else {
        return;
    };
    let last_idx = batch.len() - 1;
    for (i, (change, snapshot, version)) in batch.into_iter().enumerate() {
        let handle = BufferHandle::new(snapshot);
        if i < last_idx {
            entry.provider.apply_pending_edit(handle, &change, version);
        } else {
            entry.provider.on_edit(handle, &change, version);
        }
    }
}

fn process(job: Job, entries: &mut HashMap<BufferId, Entry>) {
    match job {
        Job::Attach {
            buffer_id,
            mut provider,
            snapshot,
            sink,
            initial_viewport,
        } => {
            // 先把 hint 注入 provider，再调 attach——这样 attach 内部的 `run_full`
            // 立刻按 viewport 范围跑 query 并以 ReplaceRange 投递。set_viewport 在
            // tree/snapshot/sink 都还没就位时不会触发 reissue（reissue_viewport_query
            // 自带 None 守卫），所以"先 set_viewport 再 attach"的顺序是安全的。
            if initial_viewport.is_some() {
                provider.set_viewport(initial_viewport);
            }
            let handle = BufferHandle::new(snapshot);
            provider.attach(handle, sink.clone());
            entries.insert(buffer_id, Entry { provider, sink });
        }
        Job::Edit {
            buffer_id,
            change,
            snapshot,
            new_version,
        } => {
            if let Some(entry) = entries.get_mut(&buffer_id) {
                let handle = BufferHandle::new(snapshot);
                entry.provider.on_edit(handle, &change, new_version);
            }
        }
        Job::Detach { buffer_id } => {
            if let Some(mut entry) = entries.remove(&buffer_id) {
                entry.provider.detach();
                entry.sink.close();
            }
        }
        Job::SetViewport {
            buffer_id,
            byte_range,
        } => {
            if let Some(entry) = entries.get_mut(&buffer_id) {
                entry.provider.set_viewport(byte_range);
            }
        }
    }
}

fn log_panic(payload: Box<dyn std::any::Any + Send>) {
    let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<未知 panic payload>".to_string()
    };
    eprintln!("[zom-syntax-worker] 高亮提供者 panic 已隔离：{msg}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::LanguageId;
    use std::sync::atomic::AtomicUsize;
    use zom_engine::{Buffer, BufferConfig, ByteOffset, TextRange};

    /// 计数 + 可阻塞的 mock provider：让我们能让 attach 阻塞，凑齐"在 channel
    /// 里多堆几个 Edit 再放行"的局面，把 worker 的 coalesce 路径逼出来。
    struct CountingProvider {
        attach_gate: Arc<(Mutex<bool>, Condvar)>,
        attach_count: Arc<AtomicUsize>,
        on_edit_count: Arc<AtomicUsize>,
        apply_pending_count: Arc<AtomicUsize>,
    }

    impl super::super::provider::HighlightProvider for CountingProvider {
        fn language(&self) -> LanguageId {
            LanguageId::new("test")
        }
        fn attach(
            &mut self,
            _buffer: super::super::provider::BufferHandle,
            _sink: super::super::sink::HighlightSink,
        ) {
            self.attach_count.fetch_add(1, Ordering::SeqCst);
            // 在测试里我们 explicit 打开 gate；生产 provider 当然不会阻塞 attach。
            let (lock, cvar) = &*self.attach_gate;
            let mut go = lock.lock().expect("attach gate 互斥锁中毒");
            while !*go {
                go = cvar.wait(go).expect("attach gate 条件变量中毒");
            }
        }
        fn on_edit(
            &mut self,
            _buffer: super::super::provider::BufferHandle,
            _change: &ChangeSet,
            _version: BufferVersion,
        ) {
            self.on_edit_count.fetch_add(1, Ordering::SeqCst);
        }
        fn apply_pending_edit(
            &mut self,
            _buffer: super::super::provider::BufferHandle,
            _change: &ChangeSet,
            _version: BufferVersion,
        ) {
            self.apply_pending_count.fetch_add(1, Ordering::SeqCst);
        }
        fn detach(&mut self) {}
    }

    fn open_gate(gate: &Arc<(Mutex<bool>, Condvar)>) {
        let (lock, cvar) = &**gate;
        let mut go = lock.lock().expect("attach gate 互斥锁中毒");
        *go = true;
        cvar.notify_all();
    }

    fn make_edit(version: BufferVersion) -> (ChangeSet, Snapshot, BufferVersion) {
        // bench 路径里 Edit 携带的 change + snapshot 都来自真实编辑事件。本测试只
        // 关心调度计数，让 buffer 给出真实的事件即可——CountingProvider 自己不解
        // 析这些字段。
        let mut buffer =
            Buffer::from_text("fn x() {}\n".to_string(), BufferConfig::default()).unwrap();
        buffer
            .insert(ByteOffset::new(0), " ")
            .expect("插入必须成功");
        let events = buffer.take_pending_events();
        let event = events.into_iter().next().expect("插入必须产生事件");
        let _ = version; // 由调用方覆盖
        (
            event.changeset().clone(),
            buffer.snapshot(),
            event.new_version(),
        )
    }

    /// 连续 5 个 Edit 都在 channel 里就绪后再放行 worker：worker 应当把它们折叠成
    /// 4 次 apply_pending_edit + 1 次 on_edit。
    #[test]
    fn worker_coalesces_consecutive_edits_for_same_buffer() {
        let attach_gate = Arc::new((Mutex::new(false), Condvar::new()));
        let attach_count = Arc::new(AtomicUsize::new(0));
        let on_edit_count = Arc::new(AtomicUsize::new(0));
        let apply_pending_count = Arc::new(AtomicUsize::new(0));

        let provider = Box::new(CountingProvider {
            attach_gate: Arc::clone(&attach_gate),
            attach_count: Arc::clone(&attach_count),
            on_edit_count: Arc::clone(&on_edit_count),
            apply_pending_count: Arc::clone(&apply_pending_count),
        });
        let handle = Arc::new(SyntaxWorkerHandle::spawn());
        let sink = super::super::sink::HighlightSink::new();
        let buf_id = crate::BufferId::from_raw(42);

        // 用任意 snapshot 起手；CountingProvider attach 进入阻塞。
        let bootstrap_buffer = Buffer::from_text(String::new(), BufferConfig::default()).unwrap();
        handle.attach(buf_id, provider, bootstrap_buffer.snapshot(), sink, None);

        // 把 5 条 Edit 全部塞进 channel——此时 worker 还卡在 attach 的 gate 上，
        // 不会处理它们。
        for _ in 0..5 {
            let (change, snapshot, version) = make_edit(BufferVersion::new(1));
            handle.edit(buf_id, change, snapshot, version);
        }

        // 放行 attach。下一轮 recv 抽干 channel 时会看到全部 5 个 Edit 一并折叠。
        open_gate(&attach_gate);
        handle.wait_for_idle();

        assert_eq!(
            attach_count.load(Ordering::SeqCst),
            1,
            "attach 必须只调 1 次"
        );
        assert_eq!(
            apply_pending_count.load(Ordering::SeqCst),
            4,
            "5 条 Edit 中前 4 条必须走 apply_pending_edit"
        );
        assert_eq!(
            on_edit_count.load(Ordering::SeqCst),
            1,
            "5 条 Edit 中最后一条必须走 on_edit"
        );

        handle.detach(buf_id);
        handle.wait_for_idle();
    }

    /// 跨 buffer 不折叠：A、B、A 三个 Edit 必须每个都走 on_edit。
    #[test]
    fn worker_does_not_coalesce_across_buffers() {
        let attach_gate = Arc::new((Mutex::new(true), Condvar::new())); // 不阻塞 attach
        let on_edit_count = Arc::new(AtomicUsize::new(0));
        let apply_pending_count = Arc::new(AtomicUsize::new(0));

        let handle = Arc::new(SyntaxWorkerHandle::spawn());
        for raw in [1u64, 2u64] {
            let provider = Box::new(CountingProvider {
                attach_gate: Arc::clone(&attach_gate),
                attach_count: Arc::new(AtomicUsize::new(0)),
                on_edit_count: Arc::clone(&on_edit_count),
                apply_pending_count: Arc::clone(&apply_pending_count),
            });
            let sink = super::super::sink::HighlightSink::new();
            let buf = Buffer::from_text(String::new(), BufferConfig::default()).unwrap();
            handle.attach(
                crate::BufferId::from_raw(raw),
                provider,
                buf.snapshot(),
                sink,
                None,
            );
        }
        handle.wait_for_idle();

        // A、B、A 顺序：B 切断 A 的折叠，第二个 A 必须独自走 on_edit。
        for raw in [1u64, 2u64, 1u64] {
            let (change, snapshot, version) = make_edit(BufferVersion::new(1));
            handle.edit(crate::BufferId::from_raw(raw), change, snapshot, version);
        }
        handle.wait_for_idle();

        assert_eq!(
            apply_pending_count.load(Ordering::SeqCst),
            0,
            "无连续同 buffer Edit 时不应触发 apply_pending_edit"
        );
        assert_eq!(
            on_edit_count.load(Ordering::SeqCst),
            3,
            "3 条 Edit 必须各自走 on_edit"
        );
    }

    /// 非 Edit Job 切断折叠：Edit、SetViewport、Edit 同 buffer 必须**各**走一次
    /// on_edit，中间不能因为顺序连续就被 Coalesce 误吃掉。
    #[test]
    fn worker_does_not_coalesce_across_setviewport() {
        let attach_gate = Arc::new((Mutex::new(true), Condvar::new()));
        let on_edit_count = Arc::new(AtomicUsize::new(0));
        let apply_pending_count = Arc::new(AtomicUsize::new(0));

        let provider = Box::new(CountingProvider {
            attach_gate: Arc::clone(&attach_gate),
            attach_count: Arc::new(AtomicUsize::new(0)),
            on_edit_count: Arc::clone(&on_edit_count),
            apply_pending_count: Arc::clone(&apply_pending_count),
        });
        let handle = Arc::new(SyntaxWorkerHandle::spawn());
        let sink = super::super::sink::HighlightSink::new();
        let buf_id = crate::BufferId::from_raw(7);
        let buf = Buffer::from_text(String::new(), BufferConfig::default()).unwrap();
        handle.attach(buf_id, provider, buf.snapshot(), sink, None);
        handle.wait_for_idle();

        let (c1, s1, v1) = make_edit(BufferVersion::new(1));
        handle.edit(buf_id, c1, s1, v1);
        handle.set_viewport(
            buf_id,
            Some(TextRange::new(ByteOffset::new(0), ByteOffset::new(1)).unwrap()),
        );
        let (c2, s2, v2) = make_edit(BufferVersion::new(2));
        handle.edit(buf_id, c2, s2, v2);
        handle.wait_for_idle();

        assert_eq!(
            apply_pending_count.load(Ordering::SeqCst),
            0,
            "SetViewport 必须切断 Edit 折叠"
        );
        assert_eq!(
            on_edit_count.load(Ordering::SeqCst),
            2,
            "两条 Edit 必须各自走 on_edit"
        );
    }
}
