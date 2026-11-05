//! 组合文档内的路径身份。
//!
//! 同一路径在文档内共享一份底层 Path：克隆只增加引用计数。
//! 有文件路径时按路径标识；顺序由组合文档在创建时固定。
//! 匿名 Buffer 则以稳定 buffer_id 作路径标识，与 PathKey::for_buffer 用 remote_id 承担同一职责。

use std::{
    cmp::Ordering,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use zcv_path::{ProjectPath, compare_natural_paths, compare_tree_entries};
use zcv_text::BufferId;

/// 组合文档在创建时固定的文件显示顺序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum PathOrder {
    Natural,
    Tree,
}

/// 路径身份的两种来源：项目根作用域的稳定坐标，或没有项目归属的绝对/匿名路径。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum PathIdentity {
    /// 项目搜索、项目大纲等跨根文档使用的稳定身份：根 id + 根内相对路径。
    Project(ProjectPath),
    /// 单文档 diff、匿名 Buffer 等没有项目根归属的来源。
    Unscoped { path: Arc<Path> },
}

/// excerpt 与 diff 文件在组合文档中的路径身份。
///
/// `order` 决定根内路径的自然/树排序；跨根时先按稳定根 id 排序，绝不能用绝对路径顺序。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PathKey {
    order: Option<PathOrder>,
    identity: PathIdentity,
}

impl PathKey {
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_order(path, PathOrder::Natural)
    }

    pub(crate) fn with_order(path: impl Into<PathBuf>, order: PathOrder) -> Self {
        Self {
            order: Some(order),
            identity: PathIdentity::Unscoped {
                path: Arc::from(path.into()),
            },
        }
    }

    /// 项目路径使用根 id + 相对路径作为身份；这是跨根组合文档的唯一排序输入。
    pub(crate) fn for_project_path(path: ProjectPath, order: PathOrder) -> Self {
        Self {
            order: Some(order),
            identity: PathIdentity::Project(path),
        }
    }

    /// 用 Buffer 的路径身份构造排序键。
    ///
    /// 有路径的修订/工作区来源共享文件路径；
    /// 没有文件路径时用 buffer_id 作路径标识，使匿名 Buffer 保持独立的 excerpt、锚点与实体 header 身份。
    pub(crate) fn for_buffer(path: Option<PathBuf>, buffer_id: BufferId, order: PathOrder) -> Self {
        match path {
            Some(path) => Self::with_order(path, order),
            None => Self::with_order(buffer_id.to_string(), order),
        }
    }

    /// 全序中的最小路径；用作游标零元与空路径占位。
    pub(crate) fn min() -> Self {
        Self {
            order: None,
            identity: PathIdentity::Unscoped {
                path: Arc::from(Path::new("")),
            },
        }
    }

    pub(crate) fn as_path(&self) -> &Path {
        match &self.identity {
            PathIdentity::Project(path) => path.relative().as_path(),
            PathIdentity::Unscoped { path } => path,
        }
    }

    fn cmp_identity(&self, other: &Self) -> Ordering {
        match (&self.identity, &other.identity) {
            (PathIdentity::Project(left), PathIdentity::Project(right)) => {
                left.root().cmp(&right.root()).then_with(|| {
                    compare_relative(
                        self.order,
                        left.relative().as_path(),
                        right.relative().as_path(),
                    )
                })
            }
            // 不同身份来源同时出现时保持确定性全序；项目身份优先于无作用域路径。
            (PathIdentity::Project(_), PathIdentity::Unscoped { .. }) => Ordering::Less,
            (PathIdentity::Unscoped { .. }, PathIdentity::Project(_)) => Ordering::Greater,
            (PathIdentity::Unscoped { path: left }, PathIdentity::Unscoped { path: right }) => {
                compare_relative(self.order, left, right)
            }
        }
    }
}

fn compare_relative(order: Option<PathOrder>, left: &Path, right: &Path) -> Ordering {
    match order {
        Some(PathOrder::Tree) => compare_tree_entries((left, false), (right, false)),
        None | Some(PathOrder::Natural) => compare_natural_paths(left, right),
    }
}

impl Ord for PathKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.order
            .cmp(&other.order)
            .then_with(|| self.cmp_identity(other))
    }
}

impl PartialOrd for PathKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 路径在 path_keys 表中的稳定索引。
///
/// 索引一经分配不再变化；锚点用它做紧凑身份与整数比较，避免携带 PathBuf。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PathKeyIndex(u64);

impl PathKeyIndex {
    pub(crate) const fn new(index: u64) -> Self {
        Self(index)
    }

    pub(crate) const fn get(self) -> u64 {
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
