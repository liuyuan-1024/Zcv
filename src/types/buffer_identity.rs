//! Buffer identity 类型：表达 Buffer 来源和当前对宿主可见的生命周期状态。
//!
//! 这些类型只描述身份与状态，不执行文件 I/O、reload 或冲突检测。

use std::path::{Path, PathBuf};

/// Buffer 的来源 / 生命周期类型。
///
/// 只记录身份边界：文件、URI、未命名文档与临时草稿；不承诺文件加载、编码探测、
/// reload 或保存输出。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BufferKind {
    /// 绑定本地文件路径的 Buffer；路径只是身份来源，不表示内容已经和磁盘一致。
    File { path: PathBuf },
    /// 绑定非文件 URI 的 Buffer，保留给远程文档或虚拟资源适配。
    Uri { uri: String },
    /// 尚未命名的新文档，通常可以被保存为文件。
    Untitled,
    /// 临时草稿或工具输出，不默认承诺可保存路径。
    Scratch,
}

impl BufferKind {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::File { path: path.into() }
    }

    pub fn uri(uri: impl Into<String>) -> Self {
        Self::Uri { uri: uri.into() }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File { path } => Some(path.as_path()),
            Self::Uri { .. } | Self::Untitled | Self::Scratch => None,
        }
    }

    pub fn uri_str(&self) -> Option<&str> {
        match self {
            Self::Uri { uri } => Some(uri.as_str()),
            Self::File { .. } | Self::Untitled | Self::Scratch => None,
        }
    }

    pub fn is_temporary(&self) -> bool {
        matches!(self, Self::Untitled | Self::Scratch)
    }
}

impl Default for BufferKind {
    fn default() -> Self {
        Self::Untitled
    }
}

/// Buffer 当前对宿主可见的生命周期状态。
///
/// 只暴露由 Buffer 状态机真实承载的状态；Loading / Reloading / Conflict 等
/// 待对应生命周期语义实现后再进入 public API。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BufferState {
    /// 当前文本与最近保存基线一致，且 Buffer 允许正常编辑。
    Clean,
    /// 当前文本相对最近保存基线已有变更。
    Dirty,
    /// 当前文本不可通过普通编辑入口修改；dirty 与否不由该状态表达。
    ReadOnly,
}
