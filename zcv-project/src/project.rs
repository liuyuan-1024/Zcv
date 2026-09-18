//! 项目数据层：仓库发现与 git 状态编排、项目快照。
//! 此文件是 `zcv-project` crate 的公共入口。

mod buffer_store;
mod git_store;
mod project_store;
mod search;
mod text_file;

#[cfg(test)]
#[path = "test/test_support.rs"]
mod test_support;

mod worktree;

pub use git_store::{
    GitJobPhase, GitJobStatus, GitOperationKind, GitOperationOutcome, GitStore, GitStoreEvent,
    RemoteOperationState, RepositorySnapshot, StatusEntry,
};
pub use project_store::{FileWatcherError, FileWatcherOperation, Project, ProjectEvent};
pub use search::{
    RegexSearchResult, SearchQuery, SearchQueryResult, SearchResult, regex_replacement_for_match,
    regex_replacements_in_text,
};
pub use text_file::{BomPolicy, EncodingConfig, InvalidUtf8Policy, LineEndingConfig};
pub use worktree::{WorktreeEntry, new_entry_destination, rename_destination, translate_path};
