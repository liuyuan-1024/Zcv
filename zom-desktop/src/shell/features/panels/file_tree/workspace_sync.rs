//! 文件树操作对 Workspace / ViewSet 的同步。

use std::path::{Path, PathBuf};

use zom_view::{ViewId, ViewSet};
use zom_workspace::{BufferId, ProjectTree, Workspace};

use super::FileTreeActivation;

pub(super) fn open_file(
    workspace: &mut Workspace,
    views: &mut ViewSet,
    path: PathBuf,
) -> FileTreeActivation {
    let existing = workspace.buffers().find_map(|(id, buffer)| {
        if buffer.path() == Some(path.as_path()) {
            Some(id)
        } else {
            None
        }
    });
    if let Some(buffer_id) = existing {
        let _ = workspace.set_active_buffer(buffer_id);
        focus_buffer_view(workspace, views, buffer_id);
        return FileTreeActivation::OpenedFile;
    }

    let buffer_id = match workspace.open_file(path.clone()) {
        Ok(buffer_id) => buffer_id,
        Err(error) => {
            eprintln!("打开文件失败：{}：{error}", path.display());
            return FileTreeActivation::Nothing;
        }
    };
    focus_buffer_view(workspace, views, buffer_id);
    FileTreeActivation::OpenedFile
}

/// 确保 `buffer_id` 有对应视图，并把它切成活动视图。
fn focus_buffer_view(workspace: &Workspace, views: &mut ViewSet, buffer_id: BufferId) {
    let existing_view = views.views().find_map(|(id, view)| {
        if view.buffer() == buffer_id {
            Some(id)
        } else {
            None
        }
    });
    let view_id = match existing_view {
        Some(id) => id,
        None => {
            let version = workspace
                .buffer(buffer_id)
                .expect("刚打开的缓冲区必然存在")
                .buffer()
                .version();
            views.open_view(buffer_id, version)
        }
    };
    views.set_active(view_id);
}

/// 关闭路径落在 `deleted` 之下（含 `deleted` 自身）的全部 buffer 及其视图。
pub(super) fn close_buffers_under(workspace: &mut Workspace, views: &mut ViewSet, deleted: &Path) {
    let victims: Vec<BufferId> = workspace
        .buffers()
        .filter_map(|(id, buffer)| {
            buffer
                .path()
                .filter(|path| path.starts_with(deleted))
                .map(|_| id)
        })
        .collect();
    for buffer_id in victims {
        let view_ids: Vec<ViewId> = views
            .views()
            .filter_map(|(view_id, view)| (view.buffer() == buffer_id).then_some(view_id))
            .collect();
        for view_id in view_ids {
            views.close_view(view_id);
        }
        let _ = workspace.close_buffer(buffer_id);
    }
    if let Some(buffer_id) = views.active_view().map(|view| view.buffer()) {
        let _ = workspace.set_active_buffer(buffer_id);
    }
}

fn first_visible_path(tree: &ProjectTree) -> Option<PathBuf> {
    tree.visible_rows()
        .first()
        .map(|row| row.path.to_path_buf())
}

pub(super) fn selected_path_after_deleting_active(
    tree: &ProjectTree,
    workspace: &Workspace,
) -> Option<PathBuf> {
    workspace
        .active_buffer()
        .and_then(|buffer| buffer.path())
        .map(Path::to_path_buf)
        .or_else(|| first_visible_path(tree))
}

/// 文件树移动 `old_prefix` → `new_prefix` 之后，把所有以 `old_prefix` 开头的
/// 已打开 buffer 的绑定路径一并更新到新位置，不关闭它们。
pub(super) fn rebase_buffers_under(
    workspace: &mut Workspace,
    old_prefix: &Path,
    new_prefix: &Path,
) {
    let updates: Vec<(BufferId, PathBuf)> = workspace
        .buffers()
        .filter_map(|(id, buffer)| {
            let path = buffer.path()?;
            let rest = path.strip_prefix(old_prefix).ok()?;
            Some((id, new_prefix.join(rest)))
        })
        .collect();
    for (id, new_path) in updates {
        if let Err(error) = workspace.rebind_buffer_path(id, new_path.clone()) {
            eprintln!("重绑定缓冲区路径失败：{}：{error}", new_path.display());
        }
    }
}
