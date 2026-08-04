//! zcv git 后端：git 命令行的同步封装与输出解析。

mod repository;
mod status;

pub use repository::{GitRepository, RealGitRepository};
pub use status::{DiffStat, FileStatus, GitStatus, StatusCode};
