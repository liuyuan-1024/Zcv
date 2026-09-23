//! Git 变更在普通编辑器中的投影。
//!
//! 普通编辑器不建立独立 diff 状态：
//! 这里把 GitStore 持有的 HEAD/index 修订文档和当前工作区文本注入统一 diff 实体，并把工作区文本中的冲突标记映射为编辑器 hunk。
//! app 只负责在 git/pane/editor 事件上注册订阅。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui::{App, Entity};
use zcv_buffer_diff::BufferDiffInput;
use zcv_editor::{Editor, EditorHunk};
use zcv_git::{FileStatus, GitRevision, parse_conflict_regions};
use zcv_multi_buffer::DiffFile;
use zcv_project::Project;
use zcv_text::{ByteOffset, TextRange};
use zcv_workspace::Pane;

/// 把 GitStore 的 base 文本快照推送给打开的普通编辑器。
///
/// 不接收 Workspace 实体：
/// 订阅注册时的初始回调发生在 Workspace 更新期间，读取自身实体会触发 double-lease panic。
pub fn refresh_pane_git_projection(pane: &Entity<Pane>, project: &Entity<Project>, cx: &mut App) {
    // 通用按文件 git 投影只服务能提供单文件身份的编辑器文档：
    // 预览等代理 Item 暴露同一个源编辑器，按编辑器实体去重避免重复注入；
    // 项目搜索与项目差异的多源编辑器不提供单文件路径，自然被排除。
    let mut opened = Vec::<(Entity<Editor>, PathBuf)>::new();
    let mut seen = HashSet::new();
    {
        let pane_ref = pane.read(cx);
        for item in pane_ref.tabs() {
            let Some(editor) = item.act_as::<Editor>(cx) else {
                continue;
            };
            let Some(path) = item.item_path(cx) else {
                continue;
            };
            if seen.insert(editor.entity_id()) {
                opened.push((editor, path));
            }
        }
    }
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
    let snapshot = working.read(cx).text_snapshot();
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
    let base_text = store.read(cx).revision_text(GitRevision::Head, path, cx);
    // index 参照：未提交视图（HEAD↔工作区）用它逐 hunk 判定已暂存 / 未暂存。
    let index_text = store.read(cx).revision_text(GitRevision::Index, path, cx);
    let Some(working) = editor.read(cx).multi_buffer().read(cx).singleton_source() else {
        return;
    };
    let input = BufferDiffInput {
        working,
        base_text,
        index_text,
        path: path.to_path_buf(),
        language_registry: store.read(cx).language_registry(),
        key: 0,
        // 普通编辑器只显示 gutter 差异，不提供变更块操作。
        operations: None,
    };
    // GitStore 预创建并按 (working, base, index) 共享同一 diff 实体。
    let diff = store.update(cx, |store, cx| {
        store.file_diff(&input, GitRevision::Head, GitRevision::Index, cx)
    });
    let line_count = diff
        .read(cx)
        .working()
        .read(cx)
        .text_snapshot()
        .line_count();
    let file = DiffFile {
        diff,
        display_path: path.to_path_buf(),
        excerpt_ranges: std::iter::once(0..line_count).collect(),
    };
    editor.update(cx, |editor, cx| {
        editor.set_diff_files(vec![file], cx);
    });
}

#[cfg(test)]
#[path = "test/editor_diff_tests.rs"]
mod tests;
