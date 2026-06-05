//! 工作区会话边界。
//!
//! `Workspace` 持有 buffer，`ViewSet` 持有这些 buffer 的窗口视图；二者在桌面端总是成对变更。
//! 本模块集中“打开文件 / 关闭受影响 buffer / 路径重绑定 / active view 同步”等会话动作，避免 feature model 各自手写同一套联动。

use std::path::{Path, PathBuf};

use zom_command::BubbleRequest;
use zom_engine::BufferConfig;
use zom_view::{ViewId, ViewSet};
use zom_workspace::{BufferId, Workspace};

pub(crate) struct WorkspaceSession {
    workspace: Workspace,
    views: ViewSet,
    /// 文件 / buffer 操作中产生、待落到 BubbleRuntime 的面向用户错误。
    /// 调用方在每次 session 操作后通过 [`take_bubbles`](Self::take_bubbles) 取走。
    pending_bubbles: Vec<BubbleRequest>,
}

impl WorkspaceSession {
    pub(crate) fn new(workspace: Workspace, views: ViewSet) -> Self {
        Self {
            workspace,
            views,
            pending_bubbles: Vec::new(),
        }
    }

    pub(crate) fn take_bubbles(&mut self) -> Vec<BubbleRequest> {
        std::mem::take(&mut self.pending_bubbles)
    }

    pub(crate) fn push_bubble(&mut self, request: BubbleRequest) {
        self.pending_bubbles.push(request);
    }

    pub(crate) fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub(crate) fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }

    pub(crate) fn views(&self) -> &ViewSet {
        &self.views
    }

    pub(crate) fn views_mut(&mut self) -> &mut ViewSet {
        &mut self.views
    }

    pub(crate) fn parts_mut(&mut self) -> (&mut Workspace, &mut ViewSet) {
        (&mut self.workspace, &mut self.views)
    }

    pub(crate) fn reset_project(&mut self, buffer_config: BufferConfig) {
        let engine = self.workspace.engine().clone();
        let mut workspace = Workspace::with_engine(engine);
        workspace.set_buffer_config(buffer_config);
        self.workspace = workspace;
        self.views = ViewSet::new();
    }

    pub(crate) fn open_file(&mut self, path: PathBuf) -> bool {
        let existing = self.workspace.buffers().find_map(|(id, buffer)| {
            if buffer.path() == Some(path.as_path()) {
                Some(id)
            } else {
                None
            }
        });
        if let Some(buffer_id) = existing {
            let _ = self.workspace.set_active_buffer(buffer_id);
            self.focus_buffer_view(buffer_id);
            return true;
        }

        let buffer_id = match self.workspace.open_file(path.clone()) {
            Ok(buffer_id) => buffer_id,
            Err(error) => {
                self.pending_bubbles.push(
                    BubbleRequest::error(format!("打开文件失败：{}：{error}", path.display()))
                        .dedupe("workspace.open_file"),
                );
                return false;
            }
        };
        self.focus_buffer_view(buffer_id);
        true
    }

    pub(crate) fn close_buffers_under(&mut self, deleted: &Path) {
        let victims: Vec<BufferId> = self
            .workspace
            .buffers()
            .filter_map(|(id, buffer)| {
                buffer
                    .path()
                    .filter(|path| path.starts_with(deleted))
                    .map(|_| id)
            })
            .collect();
        for buffer_id in victims {
            let view_ids: Vec<ViewId> = self
                .views
                .views()
                .filter_map(|(view_id, view)| (view.buffer() == buffer_id).then_some(view_id))
                .collect();
            for view_id in view_ids {
                self.views.close_view(view_id);
            }
            let _ = self.workspace.close_buffer(buffer_id);
        }
        if let Some(buffer_id) = self.views.active_view().map(|view| view.buffer()) {
            let _ = self.workspace.set_active_buffer(buffer_id);
        }
    }

    pub(crate) fn rebase_buffers_under(&mut self, old_prefix: &Path, new_prefix: &Path) {
        let updates: Vec<(BufferId, PathBuf)> = self
            .workspace
            .buffers()
            .filter_map(|(id, buffer)| {
                let path = buffer.path()?;
                let rest = path.strip_prefix(old_prefix).ok()?;
                Some((id, new_prefix.join(rest)))
            })
            .collect();
        for (id, new_path) in updates {
            if let Err(error) = self.workspace.rebind_buffer_path(id, new_path.clone()) {
                self.pending_bubbles.push(
                    BubbleRequest::error(format!(
                        "重绑定缓冲区路径失败：{}：{error}",
                        new_path.display()
                    ))
                    .dedupe("workspace.rebind"),
                );
            }
        }
    }

    fn focus_buffer_view(&mut self, buffer_id: BufferId) {
        let existing_view = self.views.views().find_map(|(id, view)| {
            if view.buffer() == buffer_id {
                Some(id)
            } else {
                None
            }
        });
        let view_id = match existing_view {
            Some(id) => id,
            None => {
                let version = self
                    .workspace
                    .buffer(buffer_id)
                    .expect("刚打开的缓冲区必然存在")
                    .buffer()
                    .version();
                self.views.open_view(buffer_id, version)
            }
        };
        self.views.set_active(view_id);
    }
}
