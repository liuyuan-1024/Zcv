//! 版本控制 UI crate —— 变更树面板、项目差异视图与提交历史图。

mod editor_diff;
mod git_graph;
mod graph;
mod project_diff;
mod version_control;

use gpui::App;

pub use git_graph::{GitGraphSerializedItemProvider, deploy_at as deploy_git_graph};
pub use project_diff::{
    ProjectDiffKind, ProjectDiffSerializedItemProvider, ProjectDiffView,
    deploy_at as deploy_project_diff,
};
pub use version_control::{OnOpenGitDiff, OnOpenGitGraph, VersionControlPanel};

pub use editor_diff::{
    inject_editor_diff, refresh_pane_git_projection, sync_editor_conflict_hunks,
};

/// 注册版本控制能力域的进程级 provider；在 `main` 初始化阶段调用一次。
pub fn init(cx: &mut App) {
    zcv_workspace::register_serialized_item_provider(ProjectDiffSerializedItemProvider, cx);
    zcv_workspace::register_serialized_item_provider(GitGraphSerializedItemProvider, cx);
}
