//! Git 命令行的同步封装与输出解析。
//! 此文件是 `zcv-git` crate 的公共入口。

mod diff;
mod graph;
mod repository;
mod status;

pub use diff::DiffHunkKind;
pub use graph::{GraphLayoutState, GraphLine, GraphRowLayout};
pub use repository::{
    Branch, GitCancellation, GitHunkOperation, GitRepository, GitRevision, GraphCommit, HunkEdit,
    RealGitRepository, WorkingCopySnapshot, apply_hunk_edits_to_text, init,
};
pub use status::{BranchStatus, DiffStat, FileStatus, GitStatus, StatusCode};
