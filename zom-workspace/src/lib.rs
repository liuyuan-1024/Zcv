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
mod document;
mod project_tree;
pub mod syntax;

pub use buffer_search::{
    BufferSearch, BufferSearchOptions, CurrentReplaceTarget, SearchSyncOutcome,
};
pub use document::SyntaxDocument;
pub use project_tree::{EntryKind, ProjectTree, TreeEntry, TreeRow};

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use zom_engine::{
    Buffer, BufferConfig, BufferLoadError, BufferOrigin as EngineBufferOrigin, BufferSaveError,
    ChangeSet, Delta, EngineError, Line,
};

use crate::syntax::{BufferSyntaxTreeSlot, LanguageId, LanguageRegistry, SyntaxEngine};

/// workspace 自己的缓冲区标识，与 `zom_engine::BufferId` 区分。
///
/// 单独建模的理由：把 engine 类型挡在 workspace 这一层；
/// view / command / desktop 讨论「哪个缓冲区」时只 `use zom_workspace::BufferId`，不被迫拉 engine 依赖。
/// 语义上 engine ID 是「哪个引擎对象」，workspace ID 是「宿主第几个槽位」。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BufferId(u64);

impl BufferId {
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// 测试 / bench 用：从原始 u64 直接构造。
    /// 生产代码请走 [`crate::syntax::SyntaxEngine::allocate_buffer_id`] 以保证全局唯一。
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
/// 语言注册表、后台 worker 与 buffer id 分配器都收口在共享的 [`SyntaxEngine`] 上（通过 `Rc` 与同进程内的其他容器——例如嵌入式[`SyntaxDocument`]——共享同一份资源）。
/// 组合根在 `Rc::new(SyntaxEngine)` 之前用 [`SyntaxEngine::registry_mut`] 注一遍内置 provider 工厂；运行期路径只读注册表。
#[derive(Debug)]
pub struct Workspace {
    engine: Rc<SyntaxEngine>,
    buffers: BTreeMap<BufferId, WorkspaceBuffer>,
    buffer_config: BufferConfig,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

impl Workspace {
    /// 便捷构造：新建一根独立的 [`SyntaxEngine`]，没有共享需求时使用。
    /// 单元测试与 file_tree 等只关心 buffer 生命周期的路径走它。
    pub fn new() -> Self {
        Self::with_engine(Rc::new(SyntaxEngine::new()))
    }

    /// 与已有 [`SyntaxEngine`] 共享：
    /// 嵌入式 [`SyntaxDocument`] 与主工作区用这条路径搭在同一根后台 worker 与同一份注册表上。
    pub fn with_engine(engine: Rc<SyntaxEngine>) -> Self {
        Self {
            engine,
            buffers: BTreeMap::new(),
            buffer_config: BufferConfig::default(),
        }
    }

    /// 当前共享的 [`SyntaxEngine`]——嵌入式文档构造时复用同一根 `Rc`。
    pub fn engine(&self) -> &Rc<SyntaxEngine> {
        &self.engine
    }

    pub fn set_buffer_config(&mut self, config: BufferConfig) {
        self.buffer_config = config.clone();
        for wb in self.buffers.values_mut() {
            wb.document.buffer_mut().set_config(config.clone());
        }
    }

    /// 后台 SyntaxWorker 句柄（轻量 clone）。测试 / bench 可用 [`crate::syntax::SyntaxWorkerHandle::wait_for_idle`] 等待异步产物。
    pub fn syntax_worker(&self) -> &std::sync::Arc<crate::syntax::SyntaxWorkerHandle> {
        self.engine.worker()
    }

    /// 语言注册表只读视图。
    pub fn language_registry(&self) -> &LanguageRegistry {
        self.engine.registry()
    }

    /// 语言注册表可变视图——组合根在启动期注入内置 provider 工厂。
    ///
    /// 只在 `Rc::new(SyntaxEngine)` 之前对其调用：一旦引擎被 `Rc` 共享，
    /// 调本方法会在运行期 panic（`Rc::get_mut` 在 strong_count > 1 时返回 `None`）。
    pub fn language_registry_mut(&mut self) -> &mut LanguageRegistry {
        Rc::get_mut(&mut self.engine)
            .expect("language_registry_mut 需在 SyntaxEngine 被 Rc 共享前调用")
            .registry_mut()
    }

    /// 从磁盘打开文件。
    ///
    /// 走 [`Buffer::from_reader`] 流式加载：一份 64 KiB 读缓冲增量喂 ropey，
    /// 不再持有"整段 bytes Vec + 整段 String + 整段 rope"三份全量内存。
    /// 详见[`zom-bench/BASELINE.md`](../../zom-bench/BASELINE.md) 的基线数据。
    pub fn open_file(&mut self, path: PathBuf) -> WorkspaceResult<BufferId> {
        let metadata = fs::metadata(&path)
            .map_err(|source| WorkspaceError::io(FileAction::ReadMetadata, &path, source))?;
        let file = fs::File::open(&path)
            .map_err(|source| WorkspaceError::io(FileAction::Read, &path, source))?;
        // BufReader 让 from_reader 的小读缓冲与 OS read syscall 解耦。
        // decoder 内部的 64 KiB 缓冲不直接撞 syscall 节奏。
        let reader = io::BufReader::with_capacity(64 * 1024, file);
        let origin = EngineBufferOrigin::external(path.to_string_lossy().into_owned());
        let mut buffer = Buffer::from_reader(origin, reader, self.buffer_config.clone())
            .map_err(|e| WorkspaceError::from_load(&path, e))?;
        if metadata.permissions().readonly() {
            buffer.set_read_only(true);
        }

        let origin = BufferOrigin::File(path);
        let wb = self.wrap_into_workspace_buffer(origin, buffer);
        let id = wb.document.buffer_id();
        self.buffers.insert(id, wb);
        Ok(id)
    }

    /// 用给定文本创建缓冲区，可选绑定路径。
    ///
    /// 本路径走 `Buffer::from_text`，不走流式 decoder——文本已是内存中的 `String`，流式收益为零。
    /// 语言识别仍读首行，与磁盘打开路径行为一致。
    pub fn open_text(
        &mut self,
        path: Option<PathBuf>,
        text: impl Into<String>,
    ) -> WorkspaceResult<BufferId> {
        let origin = match path {
            Some(path) => BufferOrigin::File(path),
            None => BufferOrigin::Scratch,
        };
        let buffer = Buffer::from_text(text.into(), self.buffer_config.clone())?;
        let wb = self.wrap_into_workspace_buffer(origin, buffer);
        let id = wb.document.buffer_id();
        self.buffers.insert(id, wb);
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

    /// 只改缓冲区的绑定路径，不写盘也不改内容。
    /// 文件树执行「移动 / 重命名」后，已打开的缓冲区用它跟随到新位置；
    /// 磁盘上的文件内容就是缓冲区里的内容，无需重读、也无需把缓冲区标记成脏。
    pub fn rebind_buffer_path(&mut self, id: BufferId, path: PathBuf) -> WorkspaceResult<()> {
        let buffer = self.buffer_mut_or_error(id)?;
        buffer.origin = BufferOrigin::File(path);
        Ok(())
    }

    /// 关闭并丢弃缓冲区。
    ///
    /// `wb` 在函数返回时 drop，会触发 [`SyntaxDocument::drop`]——provider detach 都在那里完成（手册 §九 不变量）。
    pub fn close_buffer(&mut self, id: BufferId) -> WorkspaceResult<()> {
        if self.buffers.remove(&id).is_none() {
            return Err(WorkspaceError::BufferNotFound(id));
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

    pub fn buffer_path(&self, id: BufferId) -> WorkspaceResult<Option<&Path>> {
        Ok(self.buffer_or_error(id)?.path())
    }

    pub fn is_buffer_dirty(&self, id: BufferId) -> WorkspaceResult<bool> {
        Ok(self.buffer_or_error(id)?.is_dirty())
    }

    pub fn is_buffer_read_only(&self, id: BufferId) -> WorkspaceResult<bool> {
        Ok(self.buffer_or_error(id)?.is_read_only())
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
        let wb = self.buffer_mut_or_error(id)?;
        let buffer = wb.document.buffer_mut();
        let version = buffer.version();
        let file = fs::File::create(&path)
            .map_err(|source| WorkspaceError::io(FileAction::Write, &path, source))?;
        let writer = io::BufWriter::with_capacity(64 * 1024, file);
        buffer
            .write_to(version, writer)
            .map_err(|error| WorkspaceError::from_save(&path, error))?;

        buffer.mark_saved();
        buffer.mark_synced_external();
        if rebind {
            wb.origin = BufferOrigin::File(path);
        }

        Ok(())
    }

    /// 把外部已造好的 [`Buffer`] 包成 [`WorkspaceBuffer`]——按 origin + 首行
    /// 跑一次语言识别，再把 buffer 移交给 [`SyntaxDocument`]。`open_*`
    /// 路径共用本入口，保证两条路径的语言识别 / attach 行为完全一致。
    fn wrap_into_workspace_buffer(&self, origin: BufferOrigin, buffer: Buffer) -> WorkspaceBuffer {
        let language = detect_language_for(self.engine.registry(), &origin, &buffer);
        let document = SyntaxDocument::from_buffer(self.engine.clone(), buffer, language);
        WorkspaceBuffer {
            origin,
            document,
            search: BufferSearch::new(),
        }
    }
}

/// `Workspace::open_*` 的语言识别小工厂：从 origin 取 path，从 buffer 取首行，喂给注册表。
/// [`WorkspaceBuffer::attach_syntax`] 的等价物——单 buffer 路径走 [`SyntaxDocument::from_buffer`] 把这套识别 + attach 收口在内部，不再让 WorkspaceBuffer 自己挂 provider。
fn detect_language_for(
    registry: &LanguageRegistry,
    origin: &BufferOrigin,
    buffer: &Buffer,
) -> LanguageId {
    let path = match origin {
        BufferOrigin::File(p) => Some(p.as_path()),
        BufferOrigin::Scratch => None,
    };
    let first_line = buffer
        .snapshot()
        .slice_line(Line::new(0))
        .ok()
        .map(|s| s.as_str().to_string());
    registry.detect(path, first_line.as_deref())
}

/// 一个被 workspace 持有的缓冲区，连同它的「**文件边界 + 搜索维度**」。
///
/// 由三块拼成：
/// - [`SyntaxDocument`]：`Buffer` + 高亮 layer + provider 运行态。
/// 这部分跟嵌入式编辑器**完全共用**实现——「挂语法高亮的 buffer」全工程只有一份原语，attach / pump / drop 都在 `SyntaxDocument` 上一处实现。
/// - [`BufferSearch`]：单缓冲区的搜索状态（query、命中、当前命中）。
/// 分屏看同一缓冲区的多个视图共享这一份；EditorView 阶段 2 与 panel 的「3 / 27」标签都从这里读。
/// - [`BufferOrigin`]：文件路径 / 草稿来源 / 脏状态边界。
///
/// 没有 `attach_syntax` 之类的成员——provider 在 [`SyntaxDocument::from_buffer`] 构造时就挂好。
#[derive(Debug)]
pub struct WorkspaceBuffer {
    origin: BufferOrigin,
    document: SyntaxDocument,
    search: BufferSearch,
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
        self.document.buffer().is_dirty()
    }

    pub fn is_read_only(&self) -> bool {
        self.document.buffer().is_read_only()
    }

    pub fn buffer(&self) -> &Buffer {
        self.document.buffer()
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer {
        self.document.buffer_mut()
    }

    pub fn search(&self) -> &BufferSearch {
        &self.search
    }

    pub fn search_mut(&mut self) -> &mut BufferSearch {
        &mut self.search
    }

    /// 共享的 [`BufferSyntaxTreeSlot`] —— paint 端按它现查 tree-sitter Query。
    /// `None` 表示 plain / 超阈值 / 注册表缺工厂的 buffer。
    pub fn syntax_tree_slot(&self) -> Option<&BufferSyntaxTreeSlot> {
        self.document.syntax_tree_slot()
    }

    /// 当前缓冲区 detect 出的 [`LanguageId`]。
    /// `None` 表示 plain（未挂 provider——detect 出未注册语言、文件超阈值或注册表无 factory）。
    pub fn language(&self) -> Option<LanguageId> {
        let lang = self.document.language();
        (!lang.is_plain()).then_some(lang)
    }

    /// 排空缓冲区自上次调用以来累积的 [`zom_engine::DeltaEvent`]，逐个喂给 [`BufferSearch`] 与（若挂着）syntax。
    ///
    /// **调用契约**：缓冲区发生编辑（命令派发、IME commit、replace_all 等）后**有且仅有一处**调用本方法。
    /// 不放在 [`Workspace::buffer_mut`] / `buffer()` 之类的访问器里——访问器会被调多次、调到读路径上，重复 drain 会让事件序丢失。
    /// 当前调用点：`zom-command` 在派发结束后统一调一次活动缓冲区的 `pump_post_edit`。
    pub fn pump_post_edit(&mut self) -> WorkspaceResult<bool> {
        let events = self.document.buffer_mut().take_pending_events();
        let had_events = !events.is_empty();
        for event in &events {
            self.search.apply_delta(event)?;
        }
        self.document.apply_pending_events(&events);
        Ok(had_events)
    }

    /// 推一拍 BufferSearch 状态机：收割已完成的后台搜索，必要时 spawn 新的。
    /// **非阻塞**——返回值告诉调用方"本帧是否落了新结果"以决定 reveal / repaint。
    pub fn sync_search(&mut self) -> WorkspaceResult<SearchSyncOutcome> {
        Ok(self.search.sync(self.document.buffer())?)
    }

    /// 渲染线程每帧调一次：只收割已就绪的后台搜索结果，不会主动 spawn。
    /// 没有 in-flight 时无操作（O(1) 检查），不会阻塞。
    pub fn pump_pending_search(&mut self) -> SearchSyncOutcome {
        self.search.pump_pending(self.document.buffer())
    }

    /// 替换 BufferSearch 当前命中指向的命中。无当前命中或结果集为空时返回 `Ok(None)`。
    ///
    /// 落地后**内部**调一次扇出 pump：
    /// 把缓冲区新产生的 DeltaEvent 喂回 BufferSearch 与（若挂着）语法高亮 provider；
    /// 调用方无需自己再 pump。
    pub fn replace_current_search_match(
        &mut self,
        replacement: &str,
    ) -> WorkspaceResult<Option<(Delta, ChangeSet)>> {
        let outcome = {
            // 字段级拆分借用：document 与 search 各占独立字段，borrow checker 允许。
            let buffer = self.document.buffer_mut();
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

    /// 把 BufferSearch 当前结果集中所有命中作为单个原子事务替换。无结果时返回`Ok(None)`。
    /// 同 [`Self::replace_current_search_match`] 自动扇出 pump 事件。
    pub fn replace_all_search_matches(
        &mut self,
        replacement: &str,
    ) -> WorkspaceResult<Option<(Delta, ChangeSet)>> {
        let outcome = {
            let buffer = self.document.buffer_mut();
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

    /// `pump_post_edit` 内部走的扇出 drain，单独抽出来给 replace_* 等**同方法内**触发新编辑的入口调用。
    /// 外部入口请用 `pump_post_edit`。
    fn fanout_pending_events(&mut self) -> WorkspaceResult<()> {
        let events = self.document.buffer_mut().take_pending_events();
        for event in &events {
            self.search.apply_delta(event)?;
        }
        self.document.apply_pending_events(&events);
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
