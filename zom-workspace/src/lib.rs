//! 文档 / 模型层。
//!
//! 拥有 `Buffer` 实例本身，管理缓冲区与文件的生命周期：路径、来源、
//! 脏状态、只读、保存点。不持有视图状态（滚动、光标、折叠）—— 那些
//! 属于 `zom-view`。
//!
//! 判据：同一文件开两个分屏不会不同的状态归这里（脏状态、文件路径、
//! 只读）；会不同的归 `zom-view`。
//!
//! `Workspace` 维护一个可为空的活动缓冲区指针：打开新缓冲区后自动成为活动项；
//! 关闭非活动缓冲区不影响当前活动项；关闭活动缓冲区后切到仍打开缓冲区中最近
//! 分配的一个；关闭最后一个缓冲区后活动项为空。

mod buffer_search;
mod project_tree;
pub mod syntax;

pub use buffer_search::{
    BufferSearch, BufferSearchOptions, CurrentReplaceTarget, SearchSyncOutcome,
};
pub use project_tree::{EntryKind, ProjectTree, TreeEntry, TreeRow};

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use zom_engine::{
    Buffer, BufferConfig, BufferLoadError, BufferOrigin as EngineBufferOrigin, BufferSaveError,
    ChangeSet, Delta, EngineError, MetadataLayers,
};

use crate::syntax::{
    BufferSyntaxState, HighlightSpan, LanguageRegistry, MAX_HIGHLIGHT_BYTES, SyntaxWorkerHandle,
};

/// workspace 自己的缓冲区标识，与 `zom_engine::BufferId` 区分。
///
/// 单独建模的理由：把 engine 类型挡在 workspace 这一层；view / command /
/// desktop 讨论「哪个缓冲区」时只 `use zom_workspace::BufferId`，不被迫拉 engine
/// 依赖。语义上 engine ID 是「哪个引擎对象」，workspace ID 是「宿主第几个槽位」。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BufferId(u64);

impl BufferId {
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// 测试 / bench 用：从原始 u64 直接构造。生产代码请走
    /// [`Workspace::allocate_buffer_id`] 路径以保证全局唯一。
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// 缓冲区的来源。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BufferOrigin {
    /// 绑定到磁盘文件。
    File(PathBuf),
    /// 未命名的草稿缓冲区。
    Scratch,
}

/// 工作区：当前打开的全部缓冲区的拥有者。
///
/// 拥有一个 [`LanguageRegistry`] 单例——所有缓冲区共享同一份语言识别 + provider
/// 工厂表。组合根（zom-desktop）按 Cargo feature 在启动期 `language_registry_mut`
/// 上注册 Tier 1 provider；后续接 LSP / wasm 时同样走这个入口（手册 §六 / §十）。
///
/// 还拥有一根 [`SyntaxWorkerHandle`]——单线程后台 worker，所有缓冲区的
/// provider 实例与解析 / 查询都在该线程上跑。详见
/// [改造方案 §3.2](../../zom-workspace/docs/语法高亮异步增量改造.md)。
#[derive(Debug)]
pub struct Workspace {
    next_buffer_id: u64,
    active_buffer_id: Option<BufferId>,
    buffers: BTreeMap<BufferId, WorkspaceBuffer>,
    language_registry: LanguageRegistry,
    syntax_worker: std::sync::Arc<SyntaxWorkerHandle>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            next_buffer_id: 1,
            active_buffer_id: None,
            buffers: BTreeMap::new(),
            language_registry: LanguageRegistry::new(),
            syntax_worker: std::sync::Arc::new(SyntaxWorkerHandle::spawn()),
        }
    }

    /// 后台 SyntaxWorker 句柄（轻量 clone）。测试 / bench 可用
    /// [`SyntaxWorkerHandle::wait_for_idle`] 等待异步产物。
    pub fn syntax_worker(&self) -> &std::sync::Arc<SyntaxWorkerHandle> {
        &self.syntax_worker
    }

    /// 每帧 prepaint 起手由 desktop 调一次：扫描所有缓冲区，把 worker 已就绪
    /// 的高亮产物落到各自的 [`MetadataLayers`]。
    ///
    /// 主线程开销 = 每个缓冲区一次 hashmap 查 + 一次空 drain（拿锁、看空、
    /// 放锁）。`O(buffer 数)`，单缓冲区约 µs 级，60 fps 下不掉帧。
    pub fn pump_pending_highlights(&mut self) {
        for wb in self.buffers.values_mut() {
            wb.pump_highlights();
        }
    }

    /// 把 viewport hint 转发给指定缓冲区的语法 worker——desktop 在滚动或
    /// 编辑改变可见区间时调一次，worker 据此把 `QueryCursor::set_byte_range`
    /// 限制到 viewport ± 缓冲区，每次编辑只产 `ReplaceRange` 局部段
    /// （[改造方案 §3.6](../docs/语法高亮异步增量改造.md)）。
    ///
    /// `byte_range` 通常 = 当前 viewport ± N 行（避免 capture 撕裂边界）。
    /// `None` 取消 viewport 限定，回退到全文 `ReplaceAll`。
    ///
    /// 找不到缓冲区 / 缓冲区未挂语法高亮时静默无操作——desktop 在
    /// detect 失败的 plain 缓冲区上调本方法也无副作用。
    pub fn set_buffer_viewport_hint(
        &self,
        buffer_id: BufferId,
        byte_range: Option<zom_engine::TextRange>,
    ) {
        if let Some(wb) = self.buffers.get(&buffer_id) {
            wb.set_viewport_hint(byte_range);
        }
    }

    /// 语言注册表只读视图。
    pub fn language_registry(&self) -> &LanguageRegistry {
        &self.language_registry
    }

    /// 语言注册表可变视图——组合根在启动期注入 Tier 1 provider 工厂。
    pub fn language_registry_mut(&mut self) -> &mut LanguageRegistry {
        &mut self.language_registry
    }

    /// 从磁盘打开文件。
    ///
    /// 走 [`Buffer::from_reader`] 流式加载：一份 64 KiB 读缓冲增量喂 ropey，
    /// 不再持有"整段 bytes Vec + 整段 String + 整段 rope"三份全量内存。详见
    /// [`zom-bench/BASELINE.md`](../../zom-bench/BASELINE.md) 的基线数据。
    pub fn open_file(&mut self, path: PathBuf) -> WorkspaceResult<BufferId> {
        let metadata = fs::metadata(&path)
            .map_err(|source| WorkspaceError::io(FileAction::ReadMetadata, &path, source))?;
        let file = fs::File::open(&path)
            .map_err(|source| WorkspaceError::io(FileAction::Read, &path, source))?;
        // BufReader 让 from_reader 的小读缓冲与 OS read syscall 解耦。
        // decoder 内部的 64 KiB 缓冲不直接撞 syscall 节奏。
        let reader = io::BufReader::with_capacity(64 * 1024, file);
        let origin = EngineBufferOrigin::external(path.to_string_lossy().into_owned());
        let mut buffer = Buffer::from_reader(origin, reader, BufferConfig::default())
            .map_err(|e| WorkspaceError::from_load(&path, e))?;
        if metadata.permissions().readonly() {
            buffer.set_read_only(true);
        }

        let id = self.allocate_buffer_id();
        let mut wb = WorkspaceBuffer {
            origin: BufferOrigin::File(path),
            buffer,
            search: BufferSearch::new(),
            highlight_layers: MetadataLayers::new(),
            syntax: None,
        };
        wb.attach_syntax(id, &self.language_registry, self.syntax_worker.clone());
        self.buffers.insert(id, wb);
        self.set_active_buffer_unchecked(id);
        Ok(id)
    }

    /// 用给定文本创建缓冲区，可选绑定路径。
    ///
    /// 本路径走 `Buffer::from_text`，不走流式 decoder——文本已是内存中的 `String`，
    /// 流式收益为零。语言识别仍读首行，与磁盘打开路径行为一致。
    pub fn open_text(
        &mut self,
        path: Option<PathBuf>,
        text: impl Into<String>,
    ) -> WorkspaceResult<BufferId> {
        let id = self.allocate_buffer_id();
        let origin = match path {
            Some(path) => BufferOrigin::File(path),
            None => BufferOrigin::Scratch,
        };
        let mut wb = WorkspaceBuffer {
            origin,
            buffer: Buffer::from_text(text.into(), BufferConfig::default())?,
            search: BufferSearch::new(),
            highlight_layers: MetadataLayers::new(),
            syntax: None,
        };
        wb.attach_syntax(id, &self.language_registry, self.syntax_worker.clone());
        self.buffers.insert(id, wb);
        self.set_active_buffer_unchecked(id);
        Ok(id)
    }

    /// 保存到当前绑定路径。
    pub fn save_file(&mut self, id: BufferId) -> WorkspaceResult<()> {
        let path = self
            .buffer_or_error(id)?
            .path()
            .ok_or(WorkspaceError::BufferHasNoPath(id))?
            .to_path_buf();
        self.write_buffer_to_path(id, path, false)
    }

    /// 另存为新路径，并把 buffer 重新绑定到该路径。
    pub fn save_as(&mut self, id: BufferId, path: PathBuf) -> WorkspaceResult<()> {
        self.write_buffer_to_path(id, path, true)
    }

    /// 只改缓冲区的绑定路径，不写盘也不改内容。文件树执行「移动 / 重命名」
    /// 后，已打开的缓冲区用它跟随到新位置；磁盘上的文件内容就是缓冲区里的
    /// 内容，无需重读、也无需把缓冲区标记成脏。
    pub fn rebind_buffer_path(&mut self, id: BufferId, path: PathBuf) -> WorkspaceResult<()> {
        let buffer = self.buffer_mut_or_error(id)?;
        buffer.origin = BufferOrigin::File(path);
        Ok(())
    }

    /// 关闭并丢弃缓冲区。
    pub fn close_buffer(&mut self, id: BufferId) -> WorkspaceResult<()> {
        let Some(mut wb) = self.buffers.remove(&id) else {
            return Err(WorkspaceError::BufferNotFound(id));
        };
        // detach 在缓冲区 drop 前调，确保 provider 释放底层资源并清空 layer。
        // 这里 layer 与缓冲区一并 drop，但 detach 内部仍跑一次以触发 provider 的清理钩子。
        // 手册 §九 不变量：detach 后绝不再有产物。
        if let Some(state) = wb.syntax.take() {
            state.detach(&mut wb.highlight_layers);
        }

        if self.active_buffer_id == Some(id) {
            self.active_buffer_id = self.buffers.keys().next_back().copied();
        }

        Ok(())
    }

    pub fn buffer(&self, id: BufferId) -> Option<&WorkspaceBuffer> {
        self.buffers.get(&id)
    }

    pub fn buffer_mut(&mut self, id: BufferId) -> Option<&mut WorkspaceBuffer> {
        self.buffers.get_mut(&id)
    }

    pub fn buffers(&self) -> impl Iterator<Item = (BufferId, &WorkspaceBuffer)> {
        self.buffers.iter().map(|(id, buffer)| (*id, buffer))
    }

    pub fn active_buffer_id(&self) -> Option<BufferId> {
        self.active_buffer_id
    }

    pub fn set_active_buffer(&mut self, id: BufferId) -> WorkspaceResult<()> {
        if !self.buffers.contains_key(&id) {
            return Err(WorkspaceError::BufferNotFound(id));
        }
        self.set_active_buffer_unchecked(id);
        Ok(())
    }

    pub fn active_buffer(&self) -> Option<&WorkspaceBuffer> {
        self.active_buffer_id.and_then(|id| self.buffer(id))
    }

    pub fn active_buffer_mut(&mut self) -> Option<&mut WorkspaceBuffer> {
        self.active_buffer_id.and_then(|id| self.buffer_mut(id))
    }

    pub fn buffer_path(&self, id: BufferId) -> WorkspaceResult<Option<&Path>> {
        Ok(self.buffer_or_error(id)?.path())
    }

    pub fn is_buffer_dirty(&self, id: BufferId) -> WorkspaceResult<bool> {
        Ok(self.buffer_or_error(id)?.is_dirty())
    }

    pub fn is_buffer_read_only(&self, id: BufferId) -> WorkspaceResult<bool> {
        Ok(self.buffer_or_error(id)?.is_read_only())
    }

    fn allocate_buffer_id(&mut self) -> BufferId {
        let id = BufferId(self.next_buffer_id);
        self.next_buffer_id += 1;
        id
    }

    fn set_active_buffer_unchecked(&mut self, id: BufferId) {
        self.active_buffer_id = Some(id);
    }

    fn buffer_or_error(&self, id: BufferId) -> WorkspaceResult<&WorkspaceBuffer> {
        self.buffers
            .get(&id)
            .ok_or(WorkspaceError::BufferNotFound(id))
    }

    fn buffer_mut_or_error(&mut self, id: BufferId) -> WorkspaceResult<&mut WorkspaceBuffer> {
        self.buffers
            .get_mut(&id)
            .ok_or(WorkspaceError::BufferNotFound(id))
    }

    fn write_buffer_to_path(
        &mut self,
        id: BufferId,
        path: PathBuf,
        rebind: bool,
    ) -> WorkspaceResult<()> {
        let buffer = self.buffer_mut_or_error(id)?;
        let version = buffer.buffer.version();
        let file = fs::File::create(&path)
            .map_err(|source| WorkspaceError::io(FileAction::Write, &path, source))?;
        let writer = io::BufWriter::with_capacity(64 * 1024, file);
        buffer
            .buffer
            .write_to(version, writer)
            .map_err(|error| WorkspaceError::from_save(&path, error))?;

        buffer.buffer.mark_saved();
        buffer.buffer.mark_synced_external();
        if rebind {
            buffer.origin = BufferOrigin::File(path);
        }

        Ok(())
    }
}

/// 一个被 workspace 持有的缓冲区，连同它的文件边界状态。
///
/// 还持有单缓冲区的 [`BufferSearch`]：分屏看同一缓冲区的多个视图共享
/// 这一份搜索状态（query、命中、当前命中）。EditorView 阶段 2 与 panel 的
/// "3 / 27" 标签都从这里读。
///
/// 同时持有 [`MetadataLayers<HighlightSpan>`] 与可选的 [`BufferSyntaxState`]：
/// 前者是 syntax 高亮的数据落点（手册 §三 / §十一），后者是 provider 调度运行
/// 态（手册 §七）。`syntax` 为 `None` 即 plain——detect 出未注册语言、文件
/// 超阈值、或 `make_provider` 返回 None 时落入此分支。
#[derive(Debug)]
pub struct WorkspaceBuffer {
    origin: BufferOrigin,
    buffer: Buffer,
    search: BufferSearch,
    highlight_layers: MetadataLayers<HighlightSpan>,
    syntax: Option<BufferSyntaxState>,
}

impl WorkspaceBuffer {
    pub fn origin(&self) -> &BufferOrigin {
        &self.origin
    }

    pub fn path(&self) -> Option<&Path> {
        match &self.origin {
            BufferOrigin::File(path) => Some(path.as_path()),
            BufferOrigin::Scratch => None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
    }

    pub fn is_read_only(&self) -> bool {
        self.buffer.is_read_only()
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    pub fn search(&self) -> &BufferSearch {
        &self.search
    }

    pub fn search_mut(&mut self) -> &mut BufferSearch {
        &mut self.search
    }

    /// 语法高亮 layer 的只读视图。渲染端阶段 3 按 [`syntax::syntax_layer_kind`]
    /// 取本 layer。
    pub fn highlight_layers(&self) -> &MetadataLayers<HighlightSpan> {
        &self.highlight_layers
    }

    /// 当前缓冲区 detect 出的 [`syntax::LanguageId`]。`None` 表示 plain
    /// （未挂 provider）。
    pub fn language(&self) -> Option<syntax::LanguageId> {
        self.syntax.as_ref().map(|s| s.language())
    }

    /// 排空缓冲区自上次调用以来累积的 [`zom_engine::DeltaEvent`]，逐个喂给
    /// [`BufferSearch`] 与（若挂着）[`BufferSyntaxState`]。
    ///
    /// **调用契约**：缓冲区发生编辑（命令派发、IME commit、replace_all 等）
    /// 后**有且仅有一处**调用本方法。不放在 [`Workspace::buffer_mut`] /
    /// `buffer()` 之类的访问器里——访问器会被调多次、调到读路径上，重复
    /// drain 会让事件序丢失。当前调用点：`zom-command` 在派发结束后
    /// 统一调一次活动缓冲区的 `pump_post_edit`。
    ///
    /// DeltaEvent 单一消费方契约：本方法负责一次 drain；BufferSearch 与
    /// 语法高亮 provider 各自从 ChangeSet 引用消费，**不**重新调
    /// `take_pending_events`。
    pub fn pump_post_edit(&mut self) -> WorkspaceResult<()> {
        let events = self.buffer.take_pending_events();
        for event in &events {
            self.search.apply_delta(event)?;
        }
        if let Some(state) = self.syntax.as_mut() {
            for event in &events {
                state.handle_edit(
                    &self.buffer,
                    event.changeset(),
                    event.new_version(),
                    &mut self.highlight_layers,
                );
            }
        }
        Ok(())
    }

    /// 识别当前缓冲区的语言并挂上对应 provider。
    /// 仅在创建期被 [`Workspace`] 内部调用——open_* 路径里缓冲区刚装好
    /// 内容，此时绑定语法状态正合适。
    fn attach_syntax(
        &mut self,
        buffer_id: BufferId,
        registry: &LanguageRegistry,
        worker: std::sync::Arc<SyntaxWorkerHandle>,
    ) {
        debug_assert!(self.syntax.is_none(), "重复 attach_syntax");
        if self.buffer.snapshot().len_bytes().get() > MAX_HIGHLIGHT_BYTES {
            return;
        }
        let first_line = self
            .buffer
            .snapshot()
            .slice_line(zom_engine::Line::new(0))
            .ok()
            .map(|s| s.as_str().to_string());
        let language = registry.detect(self.path(), first_line.as_deref());
        if language.is_plain() {
            return;
        }
        let Some(provider) = registry.make_provider(language) else {
            return;
        };
        let state = BufferSyntaxState::attach(
            buffer_id,
            language,
            provider,
            &self.buffer,
            &mut self.highlight_layers,
            worker,
            // workspace 不持 view，attach 时不知道 viewport——desktop 在首帧
            // render 时通过 [`crate::Workspace::set_buffer_viewport_hint`] 异步
            // 建立 hint，由 worker 内部 set_viewport 触发 viewport-scoped re-query。
            None,
        );
        self.syntax = Some(state);
    }

    /// 把后台 worker 已就绪的高亮产物 drain 到 layers——由
    /// [`Workspace::pump_pending_highlights`] 每帧驱动。
    fn pump_highlights(&mut self) {
        if let Some(state) = self.syntax.as_ref() {
            state.drain_into_layers(self.buffer.version(), &mut self.highlight_layers);
        }
    }

    /// 把 viewport hint 转发给挂在本缓冲区上的语法 worker。
    pub fn set_viewport_hint(&self, byte_range: Option<zom_engine::TextRange>) {
        if let Some(state) = self.syntax.as_ref() {
            state.set_viewport_hint(byte_range);
        }
    }

    /// 推一拍 BufferSearch 状态机：收割已完成的后台搜索，必要时 spawn 新的。
    /// **非阻塞**——返回值告诉调用方"本帧是否落了新结果"以决定 reveal / repaint。
    ///
    /// 等价于 `wb.search_mut().sync(wb.buffer())`，封一层避免借用检查器嫌弃
    /// 同时分别拿 search_mut 与 buffer。
    pub fn sync_search(&mut self) -> WorkspaceResult<SearchSyncOutcome> {
        Ok(self.search.sync(&self.buffer)?)
    }

    /// 渲染线程每帧调一次：只收割已就绪的后台搜索结果，不会主动 spawn。
    /// 没有 in-flight 时无操作（O(1) 检查），不会阻塞。
    pub fn pump_pending_search(&mut self) -> SearchSyncOutcome {
        self.search.pump_pending(&self.buffer)
    }

    /// 替换 BufferSearch 当前命中指向的命中。无当前命中或结果集
    /// 空时返回 `Ok(None)`。
    ///
    /// 落地后**内部**调一次扇出 pump：把缓冲区新产生的 DeltaEvent 喂回
    /// BufferSearch 与（若挂着）语法高亮 provider；调用方无需自己再 pump。
    pub fn replace_current_search_match(
        &mut self,
        replacement: &str,
    ) -> WorkspaceResult<Option<(Delta, ChangeSet)>> {
        let outcome = {
            // 字段级拆分借用：search 上做不可变 current_for_replace + buffer 同时可变。
            // 两个字段不重叠，borrow 检查器允许。
            let buffer = &mut self.buffer;
            let search = &mut self.search;
            let Some(target) = search.current_for_replace() else {
                return Ok(None);
            };
            if let Some(result) = target.literal() {
                buffer.replace_search_match(result, target.ordinal(), replacement)?
            } else if let Some(result) = target.regex() {
                buffer.replace_regex_match(result, target.ordinal(), replacement)?
            } else {
                None
            }
        };
        // 扇出 pump：BufferSearch try_remap 把剩余命中推进到新版本。
        // 语法高亮 provider 收到 ChangeSet 重算高亮。
        self.fanout_pending_events()?;
        Ok(outcome)
    }

    /// 把 BufferSearch 当前结果集中所有命中作为单个原子事务替换。无结果时返回
    /// `Ok(None)`。同 [`Self::replace_current_search_match`] 自动扇出 pump 事件。
    pub fn replace_all_search_matches(
        &mut self,
        replacement: &str,
    ) -> WorkspaceResult<Option<(Delta, ChangeSet)>> {
        let outcome = {
            let buffer = &mut self.buffer;
            let search = &mut self.search;
            let Some(target) = search.result_for_replace() else {
                return Ok(None);
            };
            if let Some(result) = target.literal() {
                buffer.replace_all_search_matches(result, replacement)?
            } else if let Some(result) = target.regex() {
                buffer.replace_all_regex_matches(result, replacement)?
            } else {
                None
            }
        };
        self.fanout_pending_events()?;
        Ok(outcome)
    }

    /// `pump_post_edit` 内部走的扇出 drain，单独抽出来给 replace_* 等
    /// **同方法内**触发新编辑的入口调用。外部入口请用 `pump_post_edit`。
    fn fanout_pending_events(&mut self) -> WorkspaceResult<()> {
        let events = self.buffer.take_pending_events();
        for event in &events {
            self.search.apply_delta(event)?;
        }
        if let Some(state) = self.syntax.as_mut() {
            for event in &events {
                state.handle_edit(
                    &self.buffer,
                    event.changeset(),
                    event.new_version(),
                    &mut self.highlight_layers,
                );
            }
        }
        Ok(())
    }
}

/// workspace 文件生命周期错误。
#[derive(Debug)]
pub enum WorkspaceError {
    /// engine 拒绝加载、保存文本或状态转换。
    Engine(EngineError),
    /// 文件系统读写失败。
    Io {
        action: FileAction,
        path: PathBuf,
        source: io::Error,
    },
    /// 调用方引用了未打开或已关闭的缓冲区。
    BufferNotFound(BufferId),
    /// 草稿缓冲区没有绑定路径，不能直接 `save_file`。
    BufferHasNoPath(BufferId),
}

impl WorkspaceError {
    fn io(action: FileAction, path: &Path, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }

    /// 把 `Buffer::from_reader` 失败映射到 workspace 错误：
    /// IO 变体携带具体路径，解码变体直接走 Engine。
    fn from_load(path: &Path, error: BufferLoadError) -> Self {
        match error {
            BufferLoadError::Io(source) => Self::io(FileAction::Read, path, source),
            BufferLoadError::Engine(engine) => Self::Engine(engine),
        }
    }

    /// 把 `Buffer::write_to` 失败映射到 workspace 错误：
    /// IO 变体携带具体路径，版本 / 边界变体直接走 Engine。
    fn from_save(path: &Path, error: BufferSaveError) -> Self {
        match error {
            BufferSaveError::Io(source) => Self::io(FileAction::Write, path, source),
            BufferSaveError::Engine(engine) => Self::Engine(engine),
        }
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(error) => write!(f, "{error}"),
            Self::Io {
                action,
                path,
                source,
            } => write!(f, "{}失败：{}：{}", action.label(), path.display(), source),
            Self::BufferNotFound(id) => write!(f, "缓冲区不存在：{}", id.as_u64()),
            Self::BufferHasNoPath(id) => {
                write!(f, "缓冲区未绑定文件路径：{}", id.as_u64())
            }
        }
    }
}

impl std::error::Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Engine(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::BufferNotFound(_) | Self::BufferHasNoPath(_) => None,
        }
    }
}

impl From<EngineError> for WorkspaceError {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

/// workspace 统一 Result 类型。
pub type WorkspaceResult<T> = Result<T, WorkspaceError>;

/// 文件系统动作，用于错误诊断。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileAction {
    Read,
    ReadMetadata,
    Write,
}

impl FileAction {
    fn label(self) -> &'static str {
        match self {
            Self::Read => "读取文件",
            Self::ReadMetadata => "读取文件元数据",
            Self::Write => "写入文件",
        }
    }
}
