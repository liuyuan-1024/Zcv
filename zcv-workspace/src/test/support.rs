//! 跨测试模块共享的工作区构造助手。

use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, Window};

use zcv_fs_watch::FsWatcher;
use zcv_language::LanguageRegistry;
use zcv_project::Project;

use crate::Workspace;

/// 按应用装配层的组合方式创建空工作区：无 worktree 项目 + 默认文件监视。
pub(crate) fn empty_workspace(
    languages: Arc<LanguageRegistry>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Workspace {
    let project = empty_project(languages, cx);
    Workspace::new_with_project(project, window, cx)
}

pub(crate) fn empty_project(languages: Arc<LanguageRegistry>, cx: &mut App) -> Entity<Project> {
    cx.new(|cx| Project::empty_with_watcher(languages, Arc::new(FsWatcher::new()), cx))
}
