//! 版本控制 UI crate —— 变更树面板、项目差异视图与提交历史图。

mod git_graph;
mod project_diff;
mod version_control;

pub use git_graph::deploy_at as deploy_git_graph;
pub use project_diff::{
    ProjectDiffKind, ProjectDiffSerializedItemProvider, ProjectDiffView,
    deploy_at as deploy_project_diff,
};
pub use version_control::{OnOpenGitDiff, OnOpenGitGraph, VersionControlPanel};
