//! Git 变更在普通编辑器中的投影。
//!
//! 普通编辑器不建立独立 diff 状态：
//! 这里把 GitStore 持有的 HEAD/index 修订文档和当前工作区文本注入统一 diff 实体，并把工作区文本中的冲突标记映射为编辑器 hunk。
//! app 只负责在 git/pane/editor 事件上注册订阅。

use std::path::{Path, PathBuf};

use gpui::{App, Entity};
use zcv_buffer_diff::BufferDiffInput;
use zcv_editor::{Editor, EditorHunk};
use zcv_git::{FileStatus, GitRevision, parse_conflict_regions};
use zcv_multi_buffer::DiffFile;
use zcv_project::Project;
use zcv_text::{ByteOffset, TextRange};
use zcv_workspace::Pane;

use crate::project_diff::ProjectDiffView;

/// 把 GitStore 的 base 文本快照推送给打开的普通编辑器。
///
/// 不接收 Workspace 实体：
/// 订阅注册时的初始回调发生在 Workspace 更新期间，读取自身实体会触发 double-lease panic。
pub fn refresh_pane_git_projection(pane: &Entity<Pane>, project: &Entity<Project>, cx: &mut App) {
    let opened: Vec<(Entity<Editor>, PathBuf)> = pane
        .read(cx)
        .tabs()
        .iter()
        .filter_map(|item| {
            if item.act_as::<ProjectDiffView>(cx).is_some() {
                return None;
            }
            let editor = item.act_as::<Editor>(cx)?;
            let path = item.item_path(cx)?;
            Some((editor, path))
        })
        .collect();
    for (editor, path) in &opened {
        sync_editor_conflict_hunks(editor, path, project, cx);
        inject_editor_diff(editor, path, project, cx);
    }
}

/// 让普通编辑器的文本变化只同步工作区文本上的冲突标记。
///
/// Git diff 属于 Editor 内部 MultiBuffer 的投影状态。
/// 文本编辑已经由 MultiBuffer 的源变更链路驱动 diff 重算，不能在这里重新注入，否则会把编辑器持有的 hunk 展开状态重新迁移并可能折叠。
pub fn sync_editor_conflict_hunks(
    editor: &Entity<Editor>,
    path: &Path,
    project: &Entity<Project>,
    cx: &mut App,
) {
    let is_unmerged = project
        .read(cx)
        .git_store()
        .read(cx)
        .status_for_path(path)
        .is_some_and(|entry| entry.status == FileStatus::Unmerged);
    if !is_unmerged {
        editor.update(cx, |editor, cx| editor.set_editor_hunks(Vec::new(), cx));
        return;
    }
    let Some(working) = editor.read(cx).multi_buffer().read(cx).singleton_source() else {
        return;
    };
    let snapshot = working.read(cx).text_snapshot(cx);
    let text_range =
        TextRange::new(ByteOffset::ZERO, snapshot.len_bytes()).expect("工作区文本范围必须有效");
    let text = snapshot
        .slice_text(text_range)
        .expect("工作区文本快照必须可切片")
        .to_string();
    let hunks = parse_conflict_regions(&text)
        .into_iter()
        .enumerate()
        .filter_map(|(index, region)| {
            EditorHunk::conflict(
                format!("{}\n{index}", path.display()),
                region.outer.clone(),
                region.theirs.start,
                |offset| offset,
            )
        })
        .collect();
    editor.update(cx, |editor, cx| editor.set_editor_hunks(hunks, cx));
}

/// 普通编辑器是否应注入 HEAD 差异。
///
/// 只有 index/HEAD 中的已跟踪文件可能有 HEAD 差异；
/// 未跟踪、被忽略或干净文件没有 HEAD 文本，把“缺失”当成空 base 注入会把整份工作区文本投影成新增（绿色背景）。
fn editor_diff_applies(status: Option<FileStatus>) -> bool {
    status.is_some_and(|status| matches!(status, FileStatus::Tracked { .. }))
}

/// 把单个普通编辑器的工作区源与 HEAD/index 全文统一注入。
///
/// GitStore 提供 HEAD 与 index 全文；
/// 显示 hunk 由 base/working 快照派生，并以 index 参照逐 hunk 标注已暂存 / 未暂存。
pub fn inject_editor_diff(
    editor: &Entity<Editor>,
    path: &Path,
    project: &Entity<Project>,
    cx: &mut App,
) {
    let store = project.read(cx).git_store();
    let status = store
        .read(cx)
        .status_for_path(path)
        .map(|entry| entry.status);
    if !editor_diff_applies(status) {
        editor.update(cx, |editor, cx| {
            editor.clear_diffs(cx);
        });
        return;
    }
    // HEAD/index 修订文档由 GitStore 异步提供；加载完成后重新注入。
    for revision in [GitRevision::Head, GitRevision::Index] {
        if store.read(cx).revision_document_loaded(revision, path) {
            continue;
        }
        let task = store.read(cx).load_revision_document(revision, path, cx);
        let project = project.clone();
        let editor = editor.clone();
        let path = path.to_path_buf();
        cx.spawn(async move |cx| {
            let _ = task.await;
            cx.update(|app| inject_editor_diff(&editor, &path, &project, app));
        })
        .detach();
    }
    // 主旧侧尚未加载完成时注入会把未知当成新建，等待加载回调重试。
    if !store
        .read(cx)
        .revision_document_loaded(GitRevision::Head, path)
    {
        return;
    }
    let base = store.read(cx).revision_document(GitRevision::Head, path);
    // index 参照：未提交视图（HEAD↔工作区）用它逐 hunk 判定已暂存 / 未暂存。
    let index = store.read(cx).revision_document(GitRevision::Index, path);
    let Some(working) = editor.read(cx).multi_buffer().read(cx).singleton_source() else {
        return;
    };
    let input = BufferDiffInput {
        working,
        base,
        index,
        path: path.to_path_buf(),
        // 普通编辑器只显示 gutter 差异，不提供变更块操作。
        operations: None,
    };
    // GitStore 预创建并按 (working, base, index) 共享同一 diff 实体。
    let diff = store.update(cx, |store, cx| store.file_diff(&input, cx));
    let file = DiffFile {
        diff,
        display_path: path.to_path_buf(),
        context_lines: None,
        show_file_header: false,
    };
    editor.update(cx, |editor, cx| {
        editor.set_diff_files(vec![file], cx);
    });
}

#[cfg(test)]
mod tests {
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
}
