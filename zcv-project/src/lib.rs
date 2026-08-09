//! 项目数据层：仓库发现与 git 状态编排、文件系统监听、项目快照。

mod fs_watcher;
mod project;

pub use fs_watcher::{FsWatcher, PathEvent, PathEventKind, Watcher};
pub use project::{
    GitOperationKind, GitStore, GitStoreEvent, Project, ProjectEvent, RemoteOperationState,
    RepositorySnapshot, StatusEntry, TreeRow, new_entry_destination, rename_destination,
    translate_path,
};
