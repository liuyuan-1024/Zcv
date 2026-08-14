//! git 状态 → 展示颜色的映射。
//!
//! 消费方共享层：项目树与版本控制面板共用同一套状态着色优先级。

use gpui::App;
use zcv_git::{FileStatus, StatusCode};
use zcv_theme::color;

/// git 状态 → 文本颜色（对齐 Zed `entry_git_aware_label_color` 的优先级）。
///
/// conflict > deleted > modified > added/untracked > ignored（渲染层淡显）。
pub fn git_status_color(status: FileStatus, cx: &App) -> Option<gpui::Rgba> {
    let colors = color::current(cx);
    match status {
        FileStatus::Unmerged => Some(colors.status_conflict),
        FileStatus::Untracked => Some(colors.status_created),
        FileStatus::Ignored => None,
        FileStatus::Tracked {
            index_status,
            worktree_status,
        } => {
            let deleted = matches!(index_status, StatusCode::Deleted)
                || matches!(worktree_status, StatusCode::Deleted);
            let modified = matches!(index_status, StatusCode::Modified | StatusCode::TypeChanged)
                || matches!(
                    worktree_status,
                    StatusCode::Modified | StatusCode::TypeChanged
                );
            let added = matches!(index_status, StatusCode::Added)
                || matches!(worktree_status, StatusCode::Added);
            if deleted {
                Some(colors.status_deleted)
            } else if modified {
                Some(colors.status_modified)
            } else if added {
                Some(colors.status_created)
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
    fn git_status_color_follows_zed_priority(cx: &mut gpui::TestAppContext) {
        cx.read(|cx| {
            // palette 未初始化时默认 one_dark，语义色可直接取。
            let colors = color::current(cx);
            let color = |status| git_status_color(status, cx);
            // 特殊态。
            assert_eq!(color(FileStatus::Untracked), Some(colors.status_created));
            assert_eq!(color(FileStatus::Unmerged), Some(colors.status_conflict));
            assert_eq!(color(FileStatus::Ignored), None);
            // 已跟踪：deleted > modified > added 优先级。
            let tracked = |index, worktree| FileStatus::Tracked {
                index_status: index,
                worktree_status: worktree,
            };
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Modified)),
                Some(colors.status_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Modified, StatusCode::Unmodified)),
                Some(colors.status_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::TypeChanged)),
                Some(colors.status_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Added)),
                Some(colors.status_created)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Deleted)),
                Some(colors.status_deleted)
            );
            // 部分暂存：modified 优先于 added。
            assert_eq!(
                color(tracked(StatusCode::Added, StatusCode::Modified)),
                Some(colors.status_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Unmodified)),
                None
            );
        });
    }
}
