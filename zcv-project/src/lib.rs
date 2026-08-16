//! 项目数据层：仓库发现与 git 状态编排、项目快照。

mod buffer_store;
mod git_store;
mod project;
mod worktree;

pub use git_store::{
    GitOperationKind, GitStore, GitStoreEvent, RemoteOperationState, RepositorySnapshot,
    StatusEntry,
};
pub use project::{Project, ProjectEvent};
pub use worktree::{WorktreeEntry, new_entry_destination, rename_destination, translate_path};
