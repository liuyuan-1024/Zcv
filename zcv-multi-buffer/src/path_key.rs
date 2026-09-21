//! 组合文档内的路径身份。
//!
//! 同一路径在文档内共享一份底层 Path：克隆只增加引用计数。
//! 有文件路径时按路径标识；
//! 匿名 Buffer 则以稳定 buffer_id 作路径标识，与 Zed 的 PathKey::for_buffer 用 remote_id 承担同一职责。

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use zcv_text::BufferId;

/// excerpt 与 diff 文件在组合文档中的路径身份。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PathKey(Arc<Path>);

impl PathKey {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(Arc::from(path.into()))
    }

    /// 用 Buffer 的路径身份构造排序键。
    ///
    /// 有路径的修订/工作区来源共享文件路径；
    /// 没有文件路径时用 buffer_id 作路径标识，使匿名 Buffer 保持独立的 excerpt、锚点与实体 header 身份。
    pub fn for_buffer(path: Option<PathBuf>, buffer_id: BufferId) -> Self {
        match path {
            Some(path) => Self::new(path),
            None => Self::new(buffer_id.to_string()),
        }
    }

    /// 全序中的最小路径；用作游标零元与空路径占位。
    pub fn min() -> Self {
        Self(Arc::from(Path::new("")))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// 路径在 path_keys 表中的稳定索引。
///
/// 索引一经分配不再变化；锚点用它做紧凑身份与整数比较，避免携带 PathBuf。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PathKeyIndex(u64);

impl PathKeyIndex {
    pub const fn new(index: u64) -> Self {
        Self(index)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Default for PathKey {
    fn default() -> Self {
        Self::min()
    }
}

impl From<PathBuf> for PathKey {
    fn from(path: PathBuf) -> Self {
        Self::new(path)
    }
}

impl From<&Path> for PathKey {
    fn from(path: &Path) -> Self {
        Self::new(path)
    }
}

impl AsRef<Path> for PathKey {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl fmt::Display for PathKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_path().display().fmt(f)
    }
}
