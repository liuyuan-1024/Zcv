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
