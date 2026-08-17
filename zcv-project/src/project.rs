//! 项目数据层：仓库发现与 git 状态编排、项目快照。
//! 此文件是 `zcv-project` crate 的公共入口。

mod buffer_store;
mod git_store;
mod project_store;

// 保留原内部模块路径，供 git_store/worktree 的测试辅助模块引用。
#[allow(unused_imports)]
pub(crate) use project_store as project;

mod worktree;

pub use git_store::{
    GitOperationKind, GitStore, GitStoreEvent, RemoteOperationState, RepositorySnapshot,
    StatusEntry,
};
pub use project_store::{Project, ProjectEvent};
pub use worktree::{WorktreeEntry, new_entry_destination, rename_destination, translate_path};
