//! zcv git 后端：git 命令行的同步封装与输出解析。

mod diff;
mod repository;
mod status;

pub use diff::parse_diff_hunks;
pub use repository::{GitRepository, RealGitRepository, init};
pub use status::{BranchStatus, DiffStat, FileStatus, GitStatus, StatusCode};
pub use zcv_buffer_diff::{DiffHunk, DiffHunkKind};
