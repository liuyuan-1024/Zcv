//! Git 命令行的同步封装与输出解析。
//! 此文件是 `zcv-git` crate 的公共入口。

mod diff;
mod graph;
mod repository;
mod status;

pub use diff::{DiffHunk, DiffHunkKind};
pub use graph::{GraphLayoutState, GraphLine, GraphRowLayout};
pub use repository::{
    Branch, DiffBase, GitCancellation, GitHunkOperation, GitRepository, GitRevision, GraphCommit,
    RealGitRepository, init,
};
pub use status::{BranchStatus, DiffStat, FileStatus, GitStatus, StatusCode};
