//! zom-workspace —— 文档/模型层
//!
//! 拥有 `Buffer` 实例本身，管理 buffer 与文件的生命周期：路径、origin、
//! dirty、readonly、保存点。不持有视图状态（滚动、光标、fold）—— 那些
//! 属于 `zom-view`。
//!
//! 判据：同一文件开两个分屏*不会*不同的状态归这里（dirty、文件路径、
//! 只读）；会不同的归 `zom-view`。
//!
//! `Workspace` 维护一个可为空的活动 buffer 指针：打开新 buffer 后自动成为
//! 活动项；关闭非活动 buffer 不影响当前活动项；关闭活动 buffer 后切到仍打开
//! buffer 中最近分配的一个；关闭最后一个 buffer 后活动项为空。

mod project_tree;

pub use project_tree::{EntryKind, ProjectTree, TreeEntry, TreeRow};

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use zom_engine::{Buffer, BufferConfig, BufferOrigin as EngineBufferOrigin, EngineError};

/// workspace 自己的 buffer 标识，与 `zom_engine::BufferId` 区分。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BufferId(u64);

impl BufferId {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// buffer 的来源。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BufferOrigin {
    /// 绑定到磁盘文件。
    File(PathBuf),
    /// 未命名的临时 buffer。
    Scratch,
}

/// 工作区：当前打开的全部 buffer 的拥有者。
#[derive(Debug, Default)]
pub struct Workspace {
    next_buffer_id: u64,
    active_buffer_id: Option<BufferId>,
    buffers: BTreeMap<BufferId, WorkspaceBuffer>,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            next_buffer_id: 1,
            active_buffer_id: None,
            buffers: BTreeMap::new(),
        }
    }

    /// 从磁盘打开文件。
    pub fn open_file(&mut self, path: PathBuf) -> WorkspaceResult<BufferId> {
        let bytes = fs::read(&path)
            .map_err(|source| WorkspaceError::io(FileAction::Read, &path, source))?;
        let metadata = fs::metadata(&path)
            .map_err(|source| WorkspaceError::io(FileAction::ReadMetadata, &path, source))?;
        let origin = EngineBufferOrigin::external(path.to_string_lossy().into_owned());
        let mut buffer = Buffer::from_loaded_text(origin, bytes, BufferConfig::default())?;
        if metadata.permissions().readonly() {
            buffer.set_read_only(true);
        }

        let id = self.allocate_buffer_id();
        self.buffers.insert(
            id,
            WorkspaceBuffer {
                origin: BufferOrigin::File(path),
                buffer,
            },
        );
        self.set_active_buffer_unchecked(id);
        Ok(id)
    }

    /// 用给定文本创建 buffer，可选绑定路径。
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
        let buffer = WorkspaceBuffer {
            origin,
            buffer: Buffer::from_text(text.into(), BufferConfig::default())?,
        };
        self.buffers.insert(id, buffer);
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

    /// 只改 buffer 的绑定路径，不写盘也不改内容。文件树执行"移动 / 重命名"
    /// 后，已打开的 buffer 用它跟随到新位置；磁盘上的文件内容就是 buffer 里
    /// 的内容，无需重读、也无需把 buffer 标记成 dirty。
    pub fn rebind_buffer_path(&mut self, id: BufferId, path: PathBuf) -> WorkspaceResult<()> {
        let buffer = self.buffer_mut_or_error(id)?;
        buffer.origin = BufferOrigin::File(path);
        Ok(())
    }

    /// 关闭并丢弃 buffer。
    pub fn close_buffer(&mut self, id: BufferId) -> WorkspaceResult<()> {
        if self.buffers.remove(&id).is_none() {
            return Err(WorkspaceError::BufferNotFound(id));
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
        let text = buffer.buffer.to_save_text(version)?;

        fs::write(&path, text)
            .map_err(|source| WorkspaceError::io(FileAction::Write, &path, source))?;

        buffer.buffer.mark_saved();
        buffer.buffer.mark_synced_external();
        if rebind {
            buffer.origin = BufferOrigin::File(path);
        }

        Ok(())
    }
}

/// 一个被 workspace 持有的 buffer，连同它的文件边界状态。
#[derive(Debug)]
pub struct WorkspaceBuffer {
    origin: BufferOrigin,
    buffer: Buffer,
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
    /// 调用方引用了未打开或已关闭的 buffer。
    BufferNotFound(BufferId),
    /// scratch buffer 没有绑定路径，不能直接 `save_file`。
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
            Self::BufferNotFound(id) => write!(f, "buffer 不存在：{}", id.as_u64()),
            Self::BufferHasNoPath(id) => {
                write!(f, "buffer 未绑定文件路径：{}", id.as_u64())
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
