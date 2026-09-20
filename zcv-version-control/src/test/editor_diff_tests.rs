use zcv_git::{FileStatus, StatusCode};

use super::editor_diff_applies;

/// 回归：被忽略/未跟踪文件没有 HEAD 差异，普通编辑器不得注入 HEAD 差异投影，否则缺失的 HEAD 文本会被当成空 base，把整份工作区文本投影成新增（绿色背景）。
#[test]
fn editor_diff_only_applies_to_tracked_files() {
    assert!(!editor_diff_applies(None), "状态未知/干净文件不注入");
    assert!(!editor_diff_applies(Some(FileStatus::Untracked)));
    assert!(!editor_diff_applies(Some(FileStatus::Ignored)));
    assert!(!editor_diff_applies(Some(FileStatus::Unmerged)));
    assert!(editor_diff_applies(Some(FileStatus::Tracked {
        index_status: StatusCode::Modified,
        worktree_status: StatusCode::Unmodified,
    })));
}
