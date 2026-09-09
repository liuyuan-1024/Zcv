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
mod tests {
    use super::*;

    #[gpui::test]
    fn git_status_color_follows_priority(cx: &mut gpui::TestAppContext) {
        cx.read(|cx| {
            let colors = color::current(cx);
            let color = |status| git_status_color(status, cx);
            assert_eq!(
                color(FileStatus::Untracked),
                Some(colors.version_control_added)
            );
            assert_eq!(color(FileStatus::Unmerged), Some(colors.status_conflict));
            assert_eq!(color(FileStatus::Ignored), Some(colors.text_placeholder));
            let tracked = |index, worktree| FileStatus::Tracked {
                index_status: index,
                worktree_status: worktree,
            };
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Modified)),
                Some(colors.version_control_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::TypeChanged)),
                Some(colors.version_control_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Added, StatusCode::Modified)),
                Some(colors.version_control_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Added)),
                Some(colors.version_control_added)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Deleted)),
                Some(colors.version_control_deleted)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Unmodified)),
                None
            );
        });
    }
}
