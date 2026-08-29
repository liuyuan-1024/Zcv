//! git 状态 → 展示颜色的映射。
//!
//! 项目树与版本控制面板统一使用 `version_control` 色系；
//! 优先级：conflict > deleted > modified > added/untracked > ignored（渲染层淡显）。

use gpui::App;
use zcv_git::{FileStatus, StatusCode};
use zcv_theme::color;

/// git 状态 → 文本颜色。
pub fn git_status_color(status: FileStatus, cx: &App) -> Option<gpui::Rgba> {
    let colors = color::current(cx);
    match status {
        FileStatus::Unmerged => Some(colors.status_conflict),
        FileStatus::Untracked => Some(colors.version_control_added),
        // 忽略文件用占位文本色淡显（与普通文件区分，值对应主题的 ignored 语义）。
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
    fn git_status_color_follows_zed_priority(cx: &mut gpui::TestAppContext) {
        cx.read(|cx| {
            // palette 未初始化时默认 one_dark，语义色可直接取。
            let colors = color::current(cx);
            let color = |status| git_status_color(status, cx);
            // 特殊态。
            assert_eq!(
                color(FileStatus::Untracked),
                Some(colors.version_control_added)
            );
            assert_eq!(color(FileStatus::Unmerged), Some(colors.status_conflict));
            assert_eq!(color(FileStatus::Ignored), Some(colors.text_placeholder));
            // 已跟踪：deleted > modified > added 优先级。
            let tracked = |index, worktree| FileStatus::Tracked {
                index_status: index,
                worktree_status: worktree,
            };
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Modified)),
                Some(colors.version_control_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Modified, StatusCode::Unmodified)),
                Some(colors.version_control_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::TypeChanged)),
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
            // 部分暂存：modified 优先于 added。
            assert_eq!(
                color(tracked(StatusCode::Added, StatusCode::Modified)),
                Some(colors.version_control_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Unmodified)),
                None
            );
        });
    }
}
