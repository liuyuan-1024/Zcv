//! 工作区会话边界。
//!
//! `Workspace` 持有 buffer，`ViewSet` 持有这些 buffer 的窗口视图（编辑 + 预览）；
//! 二者在桌面端总是成对变更。
//! 本模块集中"打开文件 / 关闭受影响 buffer / 路径重绑定 / 活动视图同步"等会话动作，
//! 避免 feature model 各自手写同一套联动。
//!
//! 活动视图的**唯一真相源**：[`WorkspaceSession::active_view`]，一个 `Option<ViewId>`。
//! - tab bar 高亮谁、编辑区渲染谁，全看它；
//! - 派发命令时由 App 直接喂给 `CommandContext::active_view_id`（preview view 视为无活动编辑视图）；
//! - "文件树活动文件"等"跟随 buffer"的派生量从此处投影：`active_view → view.buffer()`。

use std::path::{Path, PathBuf};

use zom_command::BubbleRequest;
use zom_engine::BufferConfig;
use zom_view::{View, ViewId, ViewSet};
use zom_workspace::{BufferId, Workspace};

pub(crate) struct WorkspaceSession {
    workspace: Workspace,
    views: ViewSet,
    /// 当前活动视图；None 表示无任何 tab。
    /// 编辑视图与预览视图同等参与"活动"语义；
    /// preview 活动时无可编辑视图。
    active_view: Option<ViewId>,
    /// 文件 / buffer 操作中产生、待落到 BubbleRuntime 的面向用户错误。
    /// 调用方在每次 session 操作后通过 [`take_bubbles`](Self::take_bubbles) 取走。
    pending_bubbles: Vec<BubbleRequest>,
}

impl WorkspaceSession {
    pub(crate) fn new(workspace: Workspace, views: ViewSet) -> Self {
        // 构造时若已有视图，以第一条作为初始活动 view（保持旧 "首条 open 默认活动" 语义）。
        let active_view = views.first_view_id();
        Self {
            workspace,
            views,
            active_view,
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

    /// 当前活动视图 id（任何 kind）。
    pub(crate) fn active_view_id(&self) -> Option<ViewId> {
        self.active_view
    }

    /// 当前活动编辑视图 id——活动视图为预览或不存在时返回 None。
    ///
    /// 命令派发前由 App 拿这个值喂给 `CommandContext::active_view_id`。
    pub(crate) fn active_edit_view_id(&self) -> Option<ViewId> {
        let id = self.active_view?;
        self.views.edit_view(id).map(|_| id)
    }

    /// 当前活动视图对应的 buffer。文件树等"跟随活动文件"的 UI 从此投影。
    pub(crate) fn active_buffer_id(&self) -> Option<BufferId> {
        self.active_view
            .and_then(|id| self.views.view(id).map(View::buffer))
    }

    /// 当前活动编辑视图的不可变借用——活动视图为预览或不存在时返回 None。
    #[cfg(test)]
    pub(crate) fn active_edit_view(&self) -> Option<&zom_view::EditView> {
        let id = self.active_view?;
        self.views.edit_view(id)
    }

    /// 当前活动编辑视图的可变借用。
    #[cfg(test)]
    pub(crate) fn active_edit_view_mut(&mut self) -> Option<&mut zom_view::EditView> {
        let id = self.active_view?;
        self.views.edit_view_mut(id)
    }

    /// 设为活动视图——view 不存在时 no-op（避免命令端拿陈旧 id 误伤）。
    pub(crate) fn set_active_view(&mut self, view_id: ViewId) {
        if self.views.view(view_id).is_some() {
            self.active_view = Some(view_id);
        }
    }

    /// 打开（或跳转到）指定 buffer 的 Markdown 预览视图。
    /// 同一 buffer 至多一条预览视图——已存在则直接激活，不重复创建。
    pub(crate) fn open_preview(&mut self, buffer_id: BufferId) {
        let view_id = self
            .views
            .find_preview_view_for_buffer(buffer_id)
            .unwrap_or_else(|| self.views.open_preview_view(buffer_id));
        self.active_view = Some(view_id);
    }

    /// 关闭指定视图。若关掉的是活动视图，按 ViewSet 中剩余顺序挑下一个候选；
    /// 编辑视图若同 buffer 还有预览，优先回退到预览（反之亦然），否则退到第一个剩余视图。
    pub(crate) fn close_view(&mut self, view_id: ViewId) {
        let was_active = self.active_view == Some(view_id);
        let same_buffer_sibling = self.views.view(view_id).and_then(|view| {
            let buffer = view.buffer();
            match view {
                View::Edit(_) => self.views.find_preview_view_for_buffer(buffer),
                View::Preview(_) => self.views.find_edit_view_for_buffer(buffer),
            }
        });
        self.views.close_view(view_id);
        if was_active {
            self.active_view = same_buffer_sibling.or_else(|| self.views.first_view_id());
        }
    }

    pub(crate) fn reset_project(&mut self, buffer_config: BufferConfig) {
        let engine = self.workspace.engine().clone();
        let mut workspace = Workspace::with_engine(engine);
        workspace.set_buffer_config(buffer_config);
        self.workspace = workspace;
        self.views = ViewSet::new();
        self.active_view = None;
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
            self.focus_buffer_edit_view(buffer_id);
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
        self.focus_buffer_edit_view(buffer_id);
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
            // 关掉本 buffer 的所有视图（编辑 + 预览），并按需调整活动视图。
            let view_ids: Vec<ViewId> = self
                .views
                .views()
                .filter_map(|(view_id, view)| (view.buffer() == buffer_id).then_some(view_id))
                .collect();
            for view_id in view_ids {
                if self.active_view == Some(view_id) {
                    self.active_view = None;
                }
                self.views.close_view(view_id);
            }
            let _ = self.workspace.close_buffer(buffer_id);
        }
        // 活动视图被关掉时挑剩余视图的第一个；都没就保持 None。
        if self.active_view.is_none() {
            self.active_view = self.views.first_view_id();
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

    fn focus_buffer_edit_view(&mut self, buffer_id: BufferId) {
        let view_id = match self.views.find_edit_view_for_buffer(buffer_id) {
            Some(id) => id,
            None => {
                let version = self
                    .workspace
                    .buffer(buffer_id)
                    .expect("刚打开的缓冲区必然存在")
                    .buffer()
                    .version();
                self.views.open_edit_view(buffer_id, version)
            }
        };
        self.active_view = Some(view_id);
    }
}
