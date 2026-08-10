//! 项目数据层：仓库发现与 git 状态编排、项目快照。

mod buffer_store;
mod git_store;
mod project;
mod worktree;

pub use project::{
    GitOperationKind, GitStore, GitStoreEvent, Project, ProjectEvent, RemoteOperationState,
    RepositorySnapshot, StatusEntry, TreeRow, new_entry_destination, rename_destination,
    translate_path,
};
