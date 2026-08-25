//! 项目数据层：仓库发现与 git 状态编排、项目快照。
//! 此文件是 `zcv-project` crate 的公共入口。

mod buffer_store;
mod git_store;
mod project_store;
mod search;

#[cfg(test)]
#[path = "test/test_support.rs"]
mod test_support;

mod worktree;

pub use git_store::{
    GitJobPhase, GitJobStatus, GitOperationKind, GitOperationOutcome, GitStore, GitStoreEvent,
    RemoteOperationState, RepositorySnapshot, StatusEntry,
};
pub use project_store::{ActiveProjectRoot, Project, ProjectEvent};
pub use search::ProjectSearchResults;
pub use worktree::{WorktreeEntry, new_entry_destination, rename_destination, translate_path};
