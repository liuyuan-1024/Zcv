//! 文件系统监听：按照 Zed 编辑器的架构实现。
//!
//! 架构分层（自底向上）：
//!
//! 1. `notify` crate —— 底层 OS 文件系统事件（FSEvents / inotify / ReadDirectoryChanges）
//! 2. `GlobalWatcher` 单例 —— 管理原生和轮询两个后端，专用线程批量调度事件
//! 3. `FsWatcher` 实例 —— 每项目根一个实例，包装 GlobalWatcher，提供 `Watcher` trait
//! 4. 调用方通过 async-channel 接收事件并触发界面刷新
//!
//! 参考：Zed crates/fs/src/fs_watcher.rs

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use crossbeam_channel::Sender as CbSender;
use notify::{Event, EventKind, RecursiveMode, Watcher as NotifyWatcher};

// ═══════════════════════════════════════════════════════════════════
// 公共类型
// ═══════════════════════════════════════════════════════════════════

/// 路径事件的类型。
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum PathEventKind {
    Removed,
    Created,
    Changed,
    /// 监听丢失了同步，消费方应全量重新扫描该路径。
    Rescan,
}

/// 文件系统路径事件。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PathEvent {
    pub path: PathBuf,
    pub kind: Option<PathEventKind>,
}

/// Watcher trait，对应 Zed 的 `Watcher` trait。
pub trait Watcher: Send + Sync {
    fn add(&self, path: &Path) -> anyhow::Result<()>;
    fn remove(&self, path: &Path) -> anyhow::Result<()>;
}

// ═══════════════════════════════════════════════════════════════════
// 内部类型
// ═══════════════════════════════════════════════════════════════════

/// 监听模式：原生 OS 监听 或 轮询。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum WatcherMode {
    #[default]
    Native,
    Poll,
}

/// 路径查找键。
///
/// 区分大小写的卷使用 `Exact`，不区分大小写的卷使用 `Folded`（小写）。
/// 两个变体是不同的键空间，Exact 查询不会误中 Folded 条目，反之亦然。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum WatchKey {
    Exact(Arc<Path>),
    Folded(Arc<str>),
}

impl WatchKey {
    fn for_path(path: &Path, case_insensitive: bool) -> Self {
        if case_insensitive {
            Self::folded(path)
        } else {
            Self::exact(path)
        }
    }

    fn exact(path: &Path) -> Self {
        Self::Exact(Arc::from(path))
    }

    fn folded(path: &Path) -> Self {
        Self::Folded(path.to_string_lossy().to_lowercase().into())
    }
}

/// 每次注册的唯一 ID。
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub(crate) struct WatcherRegistrationId(u32);

/// 对 `notify::Watcher` 的抽象，使原生和轮询两种后端可统一存储。
trait WatchBackend: Send {
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()>;
    fn unwatch(&mut self, path: &Path) -> notify::Result<()>;
}

impl<T: NotifyWatcher + Send> WatchBackend for T {
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        NotifyWatcher::watch(self, path, mode)
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        NotifyWatcher::unwatch(self, path)
    }
}

/// 调度线程上传递的事件：携带模式，用于区分来自哪个后端。
type DispatchEvent = (WatcherMode, notify::Result<notify::Event>);

/// 一次注册的状态。
struct WatcherRegistrationState {
    callback: Arc<dyn Fn(&notify::Event) + Send + Sync>,
    key: WatchKey,
    path: Arc<Path>,
    mode: WatcherMode,
}

/// 同一路径上的多个注册（多个 `FsWatcher` 实例可监听同一路径）。
struct PathRegistrationState {
    watcher_ids: Vec<WatcherRegistrationId>,
    /// 是否真正向 OS 注册了监听（还是被父目录的递归注册覆盖）。
    has_os_watcher: bool,
}

/// 按 WatchKey 索引的路径注册表。
#[derive(Default)]
struct WatchPaths(HashMap<WatchKey, PathRegistrationState>);

impl WatchPaths {
    fn contains(&self, key: &WatchKey) -> bool {
        self.0.contains_key(key)
    }

    fn get_mut(&mut self, key: &WatchKey) -> Option<&mut PathRegistrationState> {
        self.0.get_mut(key)
    }

    fn entry(
        &mut self,
        key: WatchKey,
    ) -> std::collections::hash_map::Entry<'_, WatchKey, PathRegistrationState> {
        self.0.entry(key)
    }

    fn remove(&mut self, key: &WatchKey) {
        self.0.remove(key);
    }

    /// 检查是否有祖先的递归注册已覆盖该路径。
    ///
    /// 轮询（PollWatcher）总是递归的；macOS 原生 watcher 同样按递归注册处理。
    fn covered_by_recursive_ancestor(&self, path: &Path, mode: WatcherMode) -> bool {
        if mode != WatcherMode::Poll && !cfg!(target_os = "macos") {
            return false;
        }
        for ancestor in path.ancestors().skip(1) {
            if self.0.contains_key(&WatchKey::for_path(ancestor, false))
                || self.0.contains_key(&WatchKey::for_path(ancestor, true))
            {
                return true;
            }
        }
        false
    }

    /// 收集覆盖给定路径的所有注册的 watcher ID。
    /// 同时检查 Exact 和 Folded 两种键。
    fn watcher_ids_covering(&self, path: &Path, ids: &mut Vec<WatcherRegistrationId>) {
        for ancestor in path.ancestors() {
            if let Some(reg) = self.0.get(&WatchKey::exact(ancestor)) {
                ids.extend_from_slice(&reg.watcher_ids);
            }
            if let Some(reg) = self.0.get(&WatchKey::folded(ancestor)) {
                ids.extend_from_slice(&reg.watcher_ids);
            }
        }
    }
}

/// `GlobalWatcher` 的内部状态。
struct WatcherState {
    watchers: HashMap<WatcherRegistrationId, WatcherRegistrationState>,
    native_path_registrations: WatchPaths,
    poll_path_registrations: WatchPaths,
    last_registration: WatcherRegistrationId,
}

impl WatcherState {
    fn path_registrations(&mut self, mode: WatcherMode) -> &mut WatchPaths {
        match mode {
            WatcherMode::Native => &mut self.native_path_registrations,
            WatcherMode::Poll => &mut self.poll_path_registrations,
        }
    }

    /// 移除一次注册。如果该路径上再无活跃注册且之前向 OS 注册过，则返回路径。
    fn remove_registration(
        &mut self,
        id: WatcherRegistrationId,
    ) -> Option<(Arc<Path>, WatcherMode)> {
        let registration = self.watchers.remove(&id)?;
        let path_registrations = self.path_registrations(registration.mode);
        if let Some(path_state) = path_registrations.get_mut(&registration.key) {
            path_state.watcher_ids.retain(|&existing| existing != id);
            if path_state.watcher_ids.is_empty() {
                let was_watched = path_state.has_os_watcher;
                path_registrations.remove(&registration.key);
                if was_watched {
                    return Some((registration.path, registration.mode));
                }
            }
        }
        None
    }
}

// ═══════════════════════════════════════════════════════════════════
// GlobalWatcher —— 全局单例
// ═══════════════════════════════════════════════════════════════════

/// 全局文件系统监听器单例。
///
/// - 管理原生 `notify::RecommendedWatcher` 和轮询 `notify::PollWatcher` 两个后端
/// - 提供专用调度线程，批量处理事件并对同类 Rescan 去重
/// - 所有 `FsWatcher` 实例共享此单例
pub(crate) struct GlobalWatcher {
    state: Mutex<WatcherState>,
    native_watcher: Mutex<Option<Box<dyn WatchBackend>>>,
    poll_watcher: Mutex<Option<Box<dyn WatchBackend>>>,
    event_tx: CbSender<DispatchEvent>,
}

impl GlobalWatcher {
    /// 注册一条新监听。
    fn add(
        &self,
        path: Arc<Path>,
        mode: WatcherMode,
        case_insensitive: bool,
        cb: impl Fn(&notify::Event) + Send + Sync + 'static,
    ) -> anyhow::Result<Option<WatcherRegistrationId>> {
        let key = WatchKey::for_path(&path, case_insensitive);
        let mut state = self.state.lock().unwrap();

        let (path_already_covered, path_already_registered) = {
            let regs = state.path_registrations(mode);
            (
                regs.covered_by_recursive_ancestor(&path, mode),
                regs.contains(&key),
            )
        };

        if !path_already_covered && !path_already_registered {
            // 需要在 OS 级别注册此路径
            drop(state);
            self.watch_inner(&path, mode)?;
            state = self.state.lock().unwrap();
        }

        let id = state.last_registration;
        state.last_registration = WatcherRegistrationId(id.0.wrapping_add(1));

        state.watchers.insert(
            id,
            WatcherRegistrationState {
                callback: Arc::new(cb),
                key: key.clone(),
                path: path.clone(),
                mode,
            },
        );

        state
            .path_registrations(mode)
            .entry(key)
            .and_modify(|r| r.watcher_ids.push(id))
            .or_insert_with(|| PathRegistrationState {
                watcher_ids: vec![id],
                has_os_watcher: !path_already_covered,
            });

        Ok(Some(id))
    }

    /// 移除一条监听注册。
    pub(crate) fn remove(&self, id: WatcherRegistrationId) {
        let mut state = self.state.lock().unwrap();
        if let Some((path, mode)) = state.remove_registration(id) {
            drop(state);
            self.unwatch_inner(&path, mode).ok();
        }
    }

    /// 从 notify 回调调用：将事件入队到调度线程。
    fn enqueue(&self, mode: WatcherMode, event: notify::Result<notify::Event>) {
        // 过滤 Access 事件：避免 inotify 队列溢出（Zed 的 EventKindMask::CORE 同理）
        if matches!(
            &event,
            Ok(Event {
                kind: EventKind::Access(_),
                ..
            })
        ) {
            return;
        }
        // 调度线程持有的 recv 端若已关闭则忽略
        self.event_tx.send((mode, event)).ok();
    }

    /// 批量调度一批事件。
    fn dispatch_batch(
        &self,
        first: DispatchEvent,
        rx: &crossbeam_channel::Receiver<DispatchEvent>,
    ) {
        // 每个模式在一次 batch 中只调度一次 Rescan
        let mut native_rescan = false;
        let mut poll_rescan = false;

        for (mode, event) in std::iter::once(first).chain(std::iter::from_fn(|| rx.try_recv().ok()))
        {
            let rescan_flag = match mode {
                WatcherMode::Native => &mut native_rescan,
                WatcherMode::Poll => &mut poll_rescan,
            };

            let is_rescan = event.as_ref().is_ok_and(|e| e.need_rescan());
            if is_rescan {
                if *rescan_flag {
                    continue;
                }
                *rescan_flag = true;
            }

            self.dispatch(mode, event);
        }
    }

    /// 将一条事件分发给匹配的所有注册回调。
    fn dispatch(&self, mode: WatcherMode, event: notify::Result<notify::Event>) {
        let event = match event {
            Ok(e) => e,
            Err(error) => {
                eprintln!("文件监听错误（{mode:?}）：{error}");
                return;
            }
        };

        let callbacks = {
            let state = self.state.lock().unwrap();

            if event.need_rescan() {
                // Rescan 事件：向该模式下的所有注册广播
                state
                    .watchers
                    .values()
                    .filter(|r| r.mode == mode)
                    .map(|r| r.callback.clone())
                    .collect::<Vec<_>>()
            } else {
                let registrations = match mode {
                    WatcherMode::Native => &state.native_path_registrations,
                    WatcherMode::Poll => &state.poll_path_registrations,
                };
                let mut ids = Vec::new();
                for p in &event.paths {
                    registrations.watcher_ids_covering(p, &mut ids);
                }
                ids.sort_by_key(|id| id.0);
                ids.dedup();
                ids.into_iter()
                    .filter_map(|id| state.watchers.get(&id))
                    .map(|r| r.callback.clone())
                    .collect::<Vec<_>>()
            }
        };

        for cb in callbacks {
            cb(&event);
        }
    }

    /// 向指定后端注册一条 OS 级别监听。
    fn watch_inner(&self, path: &Path, mode: WatcherMode) -> anyhow::Result<()> {
        match mode {
            WatcherMode::Native => {
                ensure_native_watcher(&self.native_watcher)?;
                self.native_watcher
                    .lock()
                    .unwrap()
                    .as_mut()
                    .expect("native watcher 已初始化")
                    .watch(
                        path,
                        if cfg!(target_os = "macos") {
                            RecursiveMode::Recursive
                        } else {
                            RecursiveMode::NonRecursive
                        },
                    )?;
            }
            WatcherMode::Poll => {
                ensure_poll_watcher(&self.poll_watcher)?;
                self.poll_watcher
                    .lock()
                    .unwrap()
                    .as_mut()
                    .expect("poll watcher 已初始化")
                    .watch(path, RecursiveMode::Recursive)?;
            }
        }
        Ok(())
    }

    /// 从指定后端移除一条 OS 级别监听。
    fn unwatch_inner(&self, path: &Path, mode: WatcherMode) -> anyhow::Result<()> {
        let result = match mode {
            WatcherMode::Native => self
                .native_watcher
                .lock()
                .unwrap()
                .as_mut()
                .map(|w| w.unwatch(path)),
            WatcherMode::Poll => self
                .poll_watcher
                .lock()
                .unwrap()
                .as_mut()
                .map(|w| w.unwatch(path)),
        };
        match result {
            // inotify 在目录被删除时自动移除监听，后续 unwatch 返回 WatchNotFound 是良性的
            Some(Err(e)) if !matches!(e.kind, notify::ErrorKind::WatchNotFound) => Err(e.into()),
            _ => Ok(()),
        }
    }
}

// 初始化原生和轮询后端的工厂函数（在 GlobalWatcher 外部以解决循环引用）。

fn ensure_native_watcher(lock: &Mutex<Option<Box<dyn WatchBackend>>>) -> anyhow::Result<()> {
    let mut guard = lock.lock().unwrap();
    if guard.is_none() {
        // recommended_watcher 按平台选择后端；
        // Access 事件不在此处过滤，统一在 enqueue() 中按事件类型过滤。
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            global_watcher().enqueue(WatcherMode::Native, event);
        })?;
        *guard = Some(Box::new(watcher));
    }
    Ok(())
}

fn ensure_poll_watcher(lock: &Mutex<Option<Box<dyn WatchBackend>>>) -> anyhow::Result<()> {
    let mut guard = lock.lock().unwrap();
    if guard.is_none() {
        let config = notify::Config::default().with_poll_interval(Duration::from_millis(2000));
        let watcher = notify::PollWatcher::new(
            move |event: notify::Result<notify::Event>| {
                global_watcher().enqueue(WatcherMode::Poll, event);
            },
            config,
        )?;
        *guard = Some(Box::new(watcher));
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════
// 全局单例
// ═══════════════════════════════════════════════════════════════════

static GLOBAL_WATCHER: OnceLock<GlobalWatcher> = OnceLock::new();

fn global_watcher() -> &'static GlobalWatcher {
    GLOBAL_WATCHER.get_or_init(|| {
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<DispatchEvent>();

        // 专用调度线程：接收来自 notify 后端的原始事件，批量分发给各注册回调
        thread::Builder::new()
            .name("fs-watcher-dispatch".into())
            .spawn(move || {
                while let Ok(first) = event_rx.recv() {
                    global_watcher().dispatch_batch(first, &event_rx);
                }
            })
            .expect("无法创建文件监听调度线程");

        GlobalWatcher {
            state: Mutex::new(WatcherState {
                watchers: HashMap::new(),
                native_path_registrations: WatchPaths::default(),
                poll_path_registrations: WatchPaths::default(),
                last_registration: WatcherRegistrationId::default(),
            }),
            native_watcher: Mutex::new(None),
            poll_watcher: Mutex::new(None),
            event_tx,
        }
    })
}

// ═══════════════════════════════════════════════════════════════════
// FsWatcher —— 每实例 watcher
// ═══════════════════════════════════════════════════════════════════

/// 调用方实例的文件系统监听器。
///
/// 包装 `GlobalWatcher` 单例，提供路径级 add/remove。
/// 事件通道与缓冲由实例自管：事件入队时发送信号，消费方经 [`FsWatcher::events`] 取得订阅对象，只写"等待并处理批次"的处理逻辑，不再自建 channel 与缓冲。
pub struct FsWatcher {
    /// 信号通道：事件入队时发送 `()`，消费方（gpui 前台 task）等待此信号。
    signal_tx: async_channel::Sender<()>,
    /// 信号接收端（订阅对象经它等待批次）。
    signal_rx: async_channel::Receiver<()>,
    /// 待处理事件的共享缓冲区。
    pending_path_events: Arc<Mutex<Vec<PathEvent>>>,
    /// 已向 GlobalWatcher 注册的路径。
    registrations: Arc<Mutex<HashMap<WatchKey, FsWatcherRegistration>>>,
    /// 等待创建的路径（路径尚不存在时由共享轮询线程等待）。
    pending_registrations: Arc<Mutex<HashMap<Arc<Path>, ()>>>,
}

#[derive(Clone, Copy)]
struct FsWatcherRegistration {
    id: WatcherRegistrationId,
    mode: WatcherMode,
}

/// 事件批次订阅：`next_batch` 等待信号并取走全部缓冲（信号合并），`has_more` 非阻塞检查是否仍有未处理的信号（防抖循环用）。
pub struct FsEventStream {
    rx: async_channel::Receiver<()>,
    pending: Arc<Mutex<Vec<PathEvent>>>,
}

impl FsEventStream {
    /// 等待下一批事件；监听器已释放（通道关闭）时返回 None。
    pub async fn next_batch(&self) -> Option<Vec<PathEvent>> {
        self.rx.recv().await.ok()?;
        Some(std::mem::take(&mut *self.pending.lock().unwrap()))
    }

    /// 是否还有未消费的信号（事件在等待处理期间再次入队）。
    pub fn has_more(&self) -> bool {
        self.rx.try_recv().is_ok()
    }
}

impl Default for FsWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl FsWatcher {
    pub fn new() -> Self {
        let (signal_tx, signal_rx) = async_channel::unbounded();
        let watcher = Self {
            signal_tx,
            signal_rx,
            pending_path_events: Arc::new(Mutex::new(Vec::new())),
            registrations: Arc::new(Mutex::new(HashMap::new())),
            pending_registrations: Arc::new(Mutex::new(HashMap::new())),
        };
        watcher.spawn_pending_poller();
        watcher
    }

    /// 取得事件批次订阅（可多次调用；各订阅共享同一事件流）。
    pub fn events(&self) -> FsEventStream {
        FsEventStream {
            rx: self.signal_rx.clone(),
            pending: self.pending_path_events.clone(),
        }
    }

    /// 共享轮询线程：统一等待所有 pending 路径出现后注册。
    ///
    /// 每路径一个独立线程会随打开路径数线性增长（对齐 Zed 用执行器异步轮询的思路，这里保持纯 std 架构，用单个线程轮询全部 pending 路径）。
    fn spawn_pending_poller(&self) {
        let registrations = self.registrations.clone();
        let pending_regs = self.pending_registrations.clone();
        let signal_tx = self.signal_tx.clone();
        let pending_events = self.pending_path_events.clone();

        thread::Builder::new()
            .name("fs-watcher-pending".into())
            .spawn(move || {
                let interval = Duration::from_millis(2000);
                loop {
                    thread::sleep(interval);

                    let paths: Vec<Arc<Path>> =
                        pending_regs.lock().unwrap().keys().cloned().collect();
                    for poll_path in paths {
                        // 已被取消（add 后 remove/clear）：跳过，不再处理
                        if !pending_regs.lock().unwrap().contains_key(&poll_path) {
                            continue;
                        }

                        // 路径尚未出现则继续轮询
                        if !poll_path.exists() {
                            continue;
                        }

                        // macOS 文件系统不区分大小写（编译期判定，与 covered_by_recursive_ancestor 一致）。
                        let case_insensitive = cfg!(target_os = "macos");
                        let key = WatchKey::for_path(&poll_path, case_insensitive);

                        if registrations.lock().unwrap().contains_key(&key) {
                            pending_regs.lock().unwrap().remove(&poll_path);
                            continue;
                        }

                        // 路径已创建，尝试注册到 GlobalWatcher
                        match register_existing_path(
                            poll_path.clone(),
                            case_insensitive,
                            signal_tx.clone(),
                            pending_events.clone(),
                        ) {
                            Ok(Some(reg)) => {
                                let mut regs = registrations.lock().unwrap();
                                if pending_regs.lock().unwrap().remove(&poll_path).is_none() {
                                    global_watcher().remove(reg.id);
                                    continue;
                                }
                                regs.insert(key, reg);
                                // 发送 Created + Rescan 事件通知消费方
                                enqueue_path_events(
                                    &signal_tx,
                                    &pending_events,
                                    vec![
                                        PathEvent {
                                            path: poll_path.to_path_buf(),
                                            kind: Some(PathEventKind::Created),
                                        },
                                        PathEvent {
                                            path: poll_path.to_path_buf(),
                                            kind: Some(PathEventKind::Rescan),
                                        },
                                    ],
                                );
                            }
                            Ok(None) => {
                                // 全局 watcher 拒绝注册（如 watch limit），继续重试
                            }
                            Err(error) => {
                                eprintln!(
                                    "为新建路径 {:?} 注册监听失败：{error}；重试中",
                                    poll_path
                                );
                            }
                        }
                    }
                }
            })
            .expect("无法创建 pending 路径轮询线程");
    }

    /// 注册一个尚不存在的路径——由共享轮询线程等待其出现。
    fn add_pending_path(&self, path: Arc<Path>) {
        let mut pending = self.pending_registrations.lock().unwrap();
        if pending.contains_key(&path) {
            return;
        }
        pending.insert(path, ());
    }
}

impl Watcher for FsWatcher {
    fn add(&self, path: &Path) -> anyhow::Result<()> {
        eprintln!("FsWatcher::add: {:?}", path);

        let path: Arc<Path> = path.into();

        // 检查是否已被已有递归注册覆盖
        {
            let regs = self.registrations.lock().unwrap();
            if path_covered_by_recursive_registration(&regs, &path) {
                eprintln!("路径 {:?} 已被现有注册覆盖", path);
                return Ok(());
            }
        }

        let case_insensitive = cfg!(target_os = "macos");
        let key = WatchKey::for_path(&path, case_insensitive);

        {
            let regs = self.registrations.lock().unwrap();
            if regs.contains_key(&key) {
                eprintln!("路径 {:?} 已注册", path);
                return Ok(());
            }
        }

        // 路径尚不存在——交给后台轮询
        if !path.exists() {
            self.add_pending_path(path);
            return Ok(());
        }

        match register_existing_path(
            path.clone(),
            case_insensitive,
            self.signal_tx.clone(),
            self.pending_path_events.clone(),
        )? {
            Some(reg) => {
                self.registrations.lock().unwrap().insert(key, reg);
            }
            None => {
                // 注册被跳过（如 watch limit 冷却），后台重试
                eprintln!("为 {:?} 注册监听被跳过，后台重试中", path);
                self.add_pending_path(path);
            }
        }

        Ok(())
    }

    fn remove(&self, path: &Path) -> anyhow::Result<()> {
        eprintln!("FsWatcher::remove: {:?}", path);
        self.pending_registrations.lock().unwrap().remove(path);

        let case_insensitive = cfg!(target_os = "macos");
        let key = WatchKey::for_path(path, case_insensitive);
        if let Some(reg) = self.registrations.lock().unwrap().remove(&key) {
            global_watcher().remove(reg.id);
        }
        Ok(())
    }
}

impl Drop for FsWatcher {
    fn drop(&mut self) {
        // 取消所有 pending 注册
        self.pending_registrations.lock().unwrap().clear();
        // 从 GlobalWatcher 注销所有注册
        let registrations = std::mem::take(&mut *self.registrations.lock().unwrap());
        for (_, reg) in registrations {
            global_watcher().remove(reg.id);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// 辅助函数
// ═══════════════════════════════════════════════════════════════════

/// 向 GlobalWatcher 注册一条已存在的路径。
fn register_existing_path(
    path: Arc<Path>,
    case_insensitive: bool,
    signal_tx: async_channel::Sender<()>,
    pending_events: Arc<Mutex<Vec<PathEvent>>>,
) -> anyhow::Result<Option<FsWatcherRegistration>> {
    let mode = if requires_poll_watcher(&path) {
        eprintln!("为 {} 使用轮询监听", path.display());
        WatcherMode::Poll
    } else {
        WatcherMode::Native
    };

    let path_for_cb = path.clone();

    let Some(id) = global_watcher().add(
        path,
        mode,
        case_insensitive,
        move |event: &notify::Event| {
            push_notify_event(&signal_tx, &pending_events, &path_for_cb, event);
        },
    )?
    else {
        return Ok(None);
    };

    Ok(Some(FsWatcherRegistration { id, mode }))
}

/// 判断某路径是否被当前注册的递归监听覆盖。
fn path_covered_by_recursive_registration(
    registrations: &HashMap<WatchKey, FsWatcherRegistration>,
    path: &Path,
) -> bool {
    for ancestor in path.ancestors().skip(1) {
        if let Some(reg) = registrations.get(&WatchKey::for_path(ancestor, false))
            && (reg.mode == WatcherMode::Poll || cfg!(target_os = "macos"))
        {
            return true;
        }
        if let Some(reg) = registrations.get(&WatchKey::for_path(ancestor, true))
            && (reg.mode == WatcherMode::Poll || cfg!(target_os = "macos"))
        {
            return true;
        }
    }
    false
}

/// 检测路径是否需要轮询监听而非原生监听。
///
/// Linux 上检测网络/虚拟文件系统（9P、NFS、CIFS、FUSE 等）。
fn requires_poll_watcher(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        return detect_requires_poll_linux(path);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        false
    }
}

#[cfg(target_os = "linux")]
fn detect_requires_poll_linux(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = match CString::new(path.as_os_str().as_bytes()) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut stat) } != 0 {
        return false;
    }

    const V9FS_MAGIC: u64 = 0x0102_1997;
    const NFS_SUPER_MAGIC: u64 = 0x0000_6969;
    const CIFS_MAGIC: u64 = 0xFF53_4D42;
    const SMB_SUPER_MAGIC: u64 = 0x0000_517B;
    const SMB2_MAGIC: u64 = 0xFE53_4D42;
    const FUSE_SUPER_MAGIC: u64 = 0x6573_5546;

    let fs_type = stat.f_type as u64;

    matches!(
        fs_type,
        V9FS_MAGIC | NFS_SUPER_MAGIC | CIFS_MAGIC | SMB_SUPER_MAGIC | SMB2_MAGIC | FUSE_SUPER_MAGIC
    )
}

/// 将 notify 事件转换为 PathEvent 并入队。
fn push_notify_event(
    signal_tx: &async_channel::Sender<()>,
    pending_path_events: &Arc<Mutex<Vec<PathEvent>>>,
    watched_root: &Path,
    event: &notify::Event,
) {
    let kind = match event.kind {
        EventKind::Create(_) => Some(PathEventKind::Created),
        EventKind::Modify(_) => Some(PathEventKind::Changed),
        EventKind::Remove(_) => Some(PathEventKind::Removed),
        _ => None,
    };

    let mut path_events: Vec<PathEvent> = event
        .paths
        .iter()
        .filter_map(|event_path| {
            // 只保留在 watched_root 下的事件
            event_path
                .strip_prefix(watched_root)
                .ok()
                .map(|_| PathEvent {
                    path: event_path.to_path_buf(),
                    kind,
                })
        })
        .collect();

    // Rescan 标记：监听丢失同步，消费方应全量刷新
    if event.need_rescan() {
        path_events.retain(|pe| pe.path != watched_root);
        path_events.push(PathEvent {
            path: watched_root.to_path_buf(),
            kind: Some(PathEventKind::Rescan),
        });
    }

    if !path_events.is_empty() {
        enqueue_path_events(signal_tx, pending_path_events, path_events);
    }
}

/// 将路径事件排序去重后入队到 pending 缓冲区，并发送信号通知消费方。
fn enqueue_path_events(
    signal_tx: &async_channel::Sender<()>,
    pending_path_events: &Arc<Mutex<Vec<PathEvent>>>,
    mut path_events: Vec<PathEvent>,
) {
    if path_events.is_empty() {
        return;
    }

    path_events.sort();
    let mut pending = pending_path_events.lock().unwrap();
    if pending.is_empty() {
        // 首次入队时发送信号，后续事件通过 coalesce 合并，不需重复信号
        let _ = signal_tx.try_send(());
    }
    coalesce_pending_rescans(&mut pending, &mut path_events);
    extend_sorted(&mut pending, path_events);
}

/// 合并 Rescan 事件。
///
/// - 如果 pending 中已有祖先的 Rescan，则子路径的 Rescan 被覆盖
/// - 如果新事件中祖先的 Rescan 替换了 pending 中的子 Rescan
fn coalesce_pending_rescans(pending: &mut Vec<PathEvent>, events: &mut Vec<PathEvent>) {
    let has_rescan = events.iter().any(|e| e.kind == Some(PathEventKind::Rescan));
    if !has_rescan {
        return;
    }

    // 提取新事件中的 Rescan 路径，排序
    let mut new_rescan: Vec<PathBuf> = events
        .iter()
        .filter(|e| e.kind == Some(PathEventKind::Rescan))
        .map(|e| e.path.clone())
        .collect();
    new_rescan.sort();

    // 去重：如果 A 是 B 的祖先，保留 A 移除 B。
    // 字典序排序后祖先与后代相邻，dedup_by 链式比较即可；
    // Path::starts_with 按组件前缀匹配（"ab" 不视为 "a" 的后代）。
    // 注意 dedup_by 闭包参数顺序是 (当前元素, 前一个保留项)。
    new_rescan.dedup_by(|current, previous| current.starts_with(previous));

    // 移除 pending 中被新 Rescan 覆盖的条目
    new_rescan.retain(|p| {
        !pending.iter().any(|pe| {
            pe.kind == Some(PathEventKind::Rescan) && *p != pe.path && p.starts_with(&pe.path)
        })
    });

    if !new_rescan.is_empty() {
        pending.retain(|pe| {
            if pe.kind != Some(PathEventKind::Rescan) {
                return true;
            }
            !new_rescan
                .iter()
                .any(|rp| *pe.path == **rp || (pe.path != *rp && pe.path.starts_with(rp)))
        });
    }

    events.retain(|e| e.kind != Some(PathEventKind::Rescan) || new_rescan.contains(&e.path));
}

/// 将 `new` 合并到 `dst`（两者均为按 path 排序的列表），按 path 去重。
fn extend_sorted(dst: &mut Vec<PathEvent>, mut new: Vec<PathEvent>) {
    new.sort_by(|a, b| a.path.cmp(&b.path));
    new.dedup_by(|a, b| a.path == b.path);

    if dst.is_empty() {
        *dst = new;
        return;
    }

    let mut result = Vec::with_capacity(dst.len() + new.len());
    let mut i = 0;
    let mut j = 0;
    while i < dst.len() && j < new.len() {
        match dst[i].path.cmp(&new[j].path) {
            std::cmp::Ordering::Less => {
                result.push(dst[i].clone());
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                result.push(new[j].clone());
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                // 保留新事件（覆盖旧事件）
                result.push(new[j].clone());
                i += 1;
                j += 1;
            }
        }
    }
    result.extend(dst[i..].iter().cloned());
    result.extend(new[j..].iter().cloned());
    *dst = result;
}

// ═══════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn rescan(path: &str) -> PathEvent {
        PathEvent {
            path: PathBuf::from(path),
            kind: Some(PathEventKind::Rescan),
        }
    }

    fn changed(path: &str) -> PathEvent {
        PathEvent {
            path: PathBuf::from(path),
            kind: Some(PathEventKind::Changed),
        }
    }

    #[test]
    fn test_watch_key_exact_vs_folded() {
        let mixed = Path::new("/Repo/Proj");
        let lower = Path::new("/repo/proj");

        // Folded 键忽略大小写
        assert_eq!(WatchKey::folded(mixed), WatchKey::folded(lower));
        // Exact 键区分大小写
        assert_ne!(WatchKey::exact(mixed), WatchKey::exact(lower));
        // Exact 和 Folded 是不同的键空间
        assert_ne!(WatchKey::exact(mixed), WatchKey::folded(mixed));
    }

    #[test]
    fn test_coalesce_rescans() {
        // 子路径 Rescan 被 pending 中的祖先覆盖
        let mut pending = vec![rescan("/root")];
        let mut events = vec![rescan("/root/child"), rescan("/root/child/grandchild")];
        coalesce_pending_rescans(&mut pending, &mut events);
        assert_eq!(pending, vec![rescan("/root")]);
        assert!(events.is_empty());

        // 新祖先 Rescan 替换 pending 中的子 Rescan
        let mut pending = vec![changed("/other"), rescan("/root/child")];
        let mut events = vec![rescan("/root")];
        coalesce_pending_rescans(&mut pending, &mut events);
        assert_eq!(pending, vec![changed("/other")]);
        assert_eq!(events, vec![rescan("/root")]);
    }

    #[test]
    fn test_ancestor_rescan_replaces_descendant_in_batch() {
        // 同一 batch 内先后出现的 Rescan，祖先应覆盖子路径
        let mut pending = vec![];
        let mut events = vec![rescan("/root/child"), rescan("/root")];
        coalesce_pending_rescans(&mut pending, &mut events);
        assert_eq!(events, vec![rescan("/root")]);
    }

    #[test]
    fn test_unrelated_rescans_are_preserved() {
        let mut pending = vec![rescan("/root-a")];
        let mut events = vec![rescan("/root-b")];
        coalesce_pending_rescans(&mut pending, &mut events);
        assert_eq!(pending, vec![rescan("/root-a")]);
        assert_eq!(events, vec![rescan("/root-b")]);
    }

    #[test]
    fn test_extend_sorted_dedup() {
        let mut dst = vec![changed("/a"), changed("/c")];
        let new = vec![changed("/b"), changed("/c")];
        extend_sorted(&mut dst, new);
        assert_eq!(dst, vec![changed("/a"), changed("/b"), changed("/c")]);
    }

    #[test]
    fn test_fs_watcher_basic_lifecycle() {
        let watcher = FsWatcher::new();
        let temp = tempfile::tempdir().unwrap();

        // add/remove 一个存在的目录
        assert!(watcher.add(temp.path()).is_ok());
        assert!(watcher.remove(temp.path()).is_ok());
    }

    #[test]
    fn test_fs_watcher_pending_path() {
        let watcher = FsWatcher::new();
        let temp = tempfile::tempdir().unwrap();
        let nonexistent = temp.path().join("nonexistent");

        // 添加不存在的路径——应启动 pending 轮询
        assert!(watcher.add(&nonexistent).is_ok());

        // 立刻移除——应取消 pending
        assert!(watcher.remove(&nonexistent).is_ok());
    }
}
