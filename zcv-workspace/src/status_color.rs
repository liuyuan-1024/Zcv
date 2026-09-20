//! Git 状态到工作区语义色的映射。
//!
//! 项目树和版本控制面板共享这项展示规则；
//! 状态优先级由这里统一定义，各面板只负责自己的行渲染。

use gpui::App;
use zcv_git::{FileStatus, StatusCode};
use zcv_theme::color;

/// Git 状态对应的文本颜色。
pub fn git_status_color(status: FileStatus, cx: &App) -> Option<gpui::Rgba> {
    let colors = color::current(cx);
    match status {
        FileStatus::Unmerged => Some(colors.status_conflict),
        FileStatus::Untracked => Some(colors.version_control_added),
        FileStatus::Ignored => Some(colors.text_placeholder),
        FileStatus::Tracked {
            index_status,
            worktree_status,
        } => {
            let is_deleted = matches!(index_status, StatusCode::Deleted)
                || matches!(worktree_status, StatusCode::Deleted);
            let is_modified =
                matches!(index_status, StatusCode::Modified | StatusCode::TypeChanged)
                    || matches!(
                        worktree_status,
                        StatusCode::Modified | StatusCode::TypeChanged
                    );
            let is_added = matches!(index_status, StatusCode::Added)
                || matches!(worktree_status, StatusCode::Added);
            if is_deleted {
                Some(colors.version_control_deleted)
            } else if is_modified {
                Some(colors.version_control_modified)
            } else if is_added {
                Some(colors.version_control_added)
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
#[path = "test/status_color_tests.rs"]
mod tests;
