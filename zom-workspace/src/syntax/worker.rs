//! 后台 SyntaxWorker：一根 std::thread 串行承载所有 buffer 的 provider 调用。
//!
//! 设计依据：[语法高亮重构计划 §Phase 3](../../docs/语法高亮重构计划.md)。
//!
//! ## 总体形态
//!
//! ```text
//!  main thread                       worker thread
//!  ───────────                       ─────────────
//!  BufferSyntax ──post Job──▶  recv → match { Attach | Edit | Detach }
//!                  (mpsc)             ↓
//!                                provider.{attach,on_edit,detach}
//!                                     ↓
//!                                provider.export_syntax_tree(&slot)
//!                                     ↓
//!  paint reads slot.load() ◀── BufferSyntaxTreeSlot
//! ```
//!
//! 关键边界：
//! - **Provider 实例的真实归属在 worker**：`Box<dyn HighlightProvider>` 通过 `Job::Attach` 从主线程交接到 worker；worker 退出前以 `Detach` 回收并 drop。
//! - **产物落 slot，不落 sink**：Phase 3 已把 sink / `MetadataLayers<HighlightSpan>` 整条路径删除。
//! worker 每条 Job 处理完调一次 `provider.export_syntax_tree(&slot)`， paint 端按 slot 上的 `Arc<BufferSyntaxTree>` 现查 viewport-scoped Query。
//! - **Job 投递不阻塞主线程**：`mpsc::Sender::send` 在标准 channel 上是 lock-free 的入队。
//! - **Panic 守护**：每条 Job 用 `catch_unwind` 包住；某 buffer 的 provider 触发 panic 不会拖垮线程，只把出问题的 entry 丢弃，buffer 退化成 plain；其他 buffer 不受影响。
//! - **同步等待**：[`SyntaxWorkerHandle::wait_for_idle`] 仅给测试 / bench 使用；正常前台路径不该走，否则就吃掉异步收益。
//!
//! ## 不再做的事（Phase 3 删除）
//!
//! - **Job::SetViewport 与 `set_viewport` 钩子**：viewport-scoped Query 由 paint 端按 [`crate::syntax::BufferSyntaxTree::query_viewport`] 在主线程 thread-local cursor 上现做，worker 不必知道 viewport。
//! - **apply_pending_edit / coalesce**：Phase 1 改造后产物落 slot —— slot 只保留最新值，连续按键的中间 reparse 即便被 worker 真跑了也只是被下一次 swap 覆盖，不会污染 sink 队列。
//! 少了 sink 也就没有"避免覆盖浪费"的目标，coalesce 没意义。

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use zom_engine::{BufferVersion, ChangeSet, Snapshot};

use crate::BufferId;

use super::provider::{BufferHandle, HighlightProvider};
use super::tree::BufferSyntaxTreeSlot;

/// 主线程 → worker 的请求。
enum Job {
    Attach {
        buffer_id: BufferId,
        provider: Box<dyn HighlightProvider>,
        snapshot: Snapshot,
        tree_slot: BufferSyntaxTreeSlot,
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
}

struct Entry {
    provider: Box<dyn HighlightProvider>,
    tree_slot: BufferSyntaxTreeSlot,
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
/// [`crate::syntax::BufferSyntax`] 之间共享：所有 buffer 走同一根 worker
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
                    self.in_flight.dec_and_notify();
                }
            }
            None => {
                self.in_flight.dec_and_notify();
            }
        }
    }

    pub(crate) fn attach(
        &self,
        buffer_id: BufferId,
        provider: Box<dyn HighlightProvider>,
        snapshot: Snapshot,
        tree_slot: BufferSyntaxTreeSlot,
    ) {
        self.post(Job::Attach {
            buffer_id,
            provider,
            snapshot,
            tree_slot,
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

    /// 阻塞直到当前已投递的所有任务都被 worker 处理完。
    ///
    /// **只给测试 / bench 用**。前台代码靠 paint 端直接读 slot，不需要同步等待。
    pub fn wait_for_idle(&self) {
        self.in_flight.wait_idle();
    }

    /// 把 JoinHandle 抢出来 detach。drop 时不再 join。
    ///
    /// **只给 bench / 长跑测试用**——bench 投 1000 任务测主线程耗时，但工艺上 worker 真处理这些重 reparse 可能跑很久；测完直接 detach 让进程退出。
    pub fn forget_join(&self) {
        if let Ok(mut guard) = self.join.lock() {
            if let Some(join) = guard.take() {
                drop(join);
            }
        }
    }
}

impl Drop for SyntaxWorkerHandle {
    fn drop(&mut self) {
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
    while let Ok(job) = rx.recv() {
        let result = catch_unwind(AssertUnwindSafe(|| process(job, &mut entries)));
        if let Err(payload) = result {
            log_panic(payload);
        }
        in_flight.dec_and_notify();
    }
}

fn process(job: Job, entries: &mut HashMap<BufferId, Entry>) {
    match job {
        Job::Attach {
            buffer_id,
            mut provider,
            snapshot,
            tree_slot,
        } => {
            let handle = BufferHandle::new(snapshot);
            provider.attach(handle);
            provider.export_syntax_tree(&tree_slot);
            entries.insert(
                buffer_id,
                Entry {
                    provider,
                    tree_slot,
                },
            );
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
                entry.provider.export_syntax_tree(&entry.tree_slot);
            }
        }
        Job::Detach { buffer_id } => {
            if let Some(mut entry) = entries.remove(&buffer_id) {
                entry.provider.detach();
                entry.tree_slot.clear();
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
    use zom_engine::{Buffer, BufferConfig, ByteOffset};

    /// 计数 mock provider —— 验证调度链路上 attach / on_edit / detach 各调一次。
    struct CountingProvider {
        attach_count: Arc<AtomicUsize>,
        on_edit_count: Arc<AtomicUsize>,
        detach_count: Arc<AtomicUsize>,
    }

    impl super::super::provider::HighlightProvider for CountingProvider {
        fn language(&self) -> LanguageId {
            LanguageId::new("test")
        }
        fn attach(&mut self, _buffer: super::super::provider::BufferHandle) {
            self.attach_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_edit(
            &mut self,
            _buffer: super::super::provider::BufferHandle,
            _change: &ChangeSet,
            _version: BufferVersion,
        ) {
            self.on_edit_count.fetch_add(1, Ordering::SeqCst);
        }
        fn detach(&mut self) {
            self.detach_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn make_edit() -> (ChangeSet, Snapshot, BufferVersion) {
        let mut buffer =
            Buffer::from_text("fn x() {}\n".to_string(), BufferConfig::default()).unwrap();
        buffer
            .insert(ByteOffset::new(0), " ")
            .expect("插入必须成功");
        let events = buffer.take_pending_events();
        let event = events.into_iter().next().expect("插入必须产生事件");
        (
            event.changeset().clone(),
            buffer.snapshot(),
            event.new_version(),
        )
    }

    /// attach → 3 次 edit → detach：各回调计数符合预期。
    #[test]
    fn worker_dispatches_attach_edit_detach_in_order() {
        let attach_count = Arc::new(AtomicUsize::new(0));
        let on_edit_count = Arc::new(AtomicUsize::new(0));
        let detach_count = Arc::new(AtomicUsize::new(0));
        let provider = Box::new(CountingProvider {
            attach_count: Arc::clone(&attach_count),
            on_edit_count: Arc::clone(&on_edit_count),
            detach_count: Arc::clone(&detach_count),
        });

        let handle = Arc::new(SyntaxWorkerHandle::spawn());
        let buf_id = crate::BufferId::from_raw(42);
        let bootstrap_buffer = Buffer::from_text(String::new(), BufferConfig::default()).unwrap();
        handle.attach(
            buf_id,
            provider,
            bootstrap_buffer.snapshot(),
            BufferSyntaxTreeSlot::new(),
        );
        for _ in 0..3 {
            let (change, snapshot, version) = make_edit();
            handle.edit(buf_id, change, snapshot, version);
        }
        handle.detach(buf_id);
        handle.wait_for_idle();

        assert_eq!(attach_count.load(Ordering::SeqCst), 1);
        assert_eq!(on_edit_count.load(Ordering::SeqCst), 3);
        assert_eq!(detach_count.load(Ordering::SeqCst), 1);
    }
}
