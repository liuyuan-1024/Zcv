//! 主编辑区的"虚拟"owner。
//!
//! 主编辑区不像文件树 / 项目选择器那样在 feature 模型里持有独立 [`super::Editor`]
//! —— 它的 buffer 在 [`Workspace`] 里、selection 在 [`ViewSet`] 的活动视图里。
//! 路由要求统一通过 [`TextTargetOwner`] / [`TextTargetQuery`] 反查，所以这里
//! 包一个透视结构：每次方法调用都从 workspace/view 当场拼出 ImeTarget /
//! EditTarget / 快照。
//!
//! 读 / 写两个变体分别绑不同的借用强度 —— 路由的查询路径只持 `&workspace +
//! &views`，写入路径才升级到 `&mut`。

use zom_command::EditTarget;
use zom_view::ViewSet;
use zom_workspace::Workspace;

use super::ime::{ImeQueryTarget, ImeTarget};
use super::owner::{TextTargetOwner, TextTargetQuery};
use super::{EditorSnapshot, TextInputProfile, TextTargetId};

/// 写入侧：路由要 `&mut` 时构造它。
pub(crate) struct MainEditorOwner<'a> {
    workspace: &'a mut Workspace,
    views: &'a mut ViewSet,
}

/// 只读侧：路由非可变路径时构造它。
pub(crate) struct MainEditorOwnerRef<'a> {
    workspace: &'a Workspace,
    views: &'a ViewSet,
}

impl<'a> MainEditorOwner<'a> {
    pub(crate) fn new(workspace: &'a mut Workspace, views: &'a mut ViewSet) -> Self {
        Self { workspace, views }
    }
}

impl<'a> MainEditorOwnerRef<'a> {
    pub(crate) fn new(workspace: &'a Workspace, views: &'a ViewSet) -> Self {
        Self { workspace, views }
    }
}

fn snapshot_from_active_view(workspace: &Workspace, views: &ViewSet) -> EditorSnapshot {
    let Some(view) = views.active_view() else {
        return EditorSnapshot::default();
    };
    let Some(buffer) = workspace.buffer(view.buffer()) else {
        return EditorSnapshot::default();
    };
    let selection = view.selection().clone();
    // BufferSearch 由 zom-workspace 维护、per-buffer 共享。这里只读快照；
    // 重跑 / try_remap 的责任在 panel 输入流 / 编辑流（见 app.rs）。
    let search = buffer.search();
    let search_hits: Vec<zom_engine::TextRange> = search.ranges().collect();
    let search_current = search.current_range();
    EditorSnapshot {
        text: buffer.buffer().text().into_owned(),
        cursor_byte: selection.primary().head().get(),
        selection,
        reveal: view.reveal().map(|req| super::RevealHint {
            byte: req.byte.get(),
            kind: req.kind,
            seq: req.seq,
        }),
        search_hits,
        search_current,
    }
}

fn ime_query_from_active_view<'a>(
    workspace: &'a Workspace,
    views: &'a ViewSet,
) -> Option<ImeQueryTarget<'a>> {
    let view = views.active_view()?;
    let buffer = workspace.buffer(view.buffer())?.buffer();
    Some(ImeQueryTarget::new(buffer, view.selection()))
}

impl<'a> TextTargetQuery for MainEditorOwner<'a> {
    fn target_id(&self) -> TextTargetId {
        TextTargetId::MainEditor
    }

    fn is_active(&self) -> bool {
        self.views.active_view().is_some()
    }

    fn snapshot(&self) -> EditorSnapshot {
        snapshot_from_active_view(self.workspace, self.views)
    }

    fn profile(&self) -> TextInputProfile {
        TextInputProfile::MainEditor
    }

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        ime_query_from_active_view(self.workspace, self.views)
    }
}

impl<'a> TextTargetOwner for MainEditorOwner<'a> {
    fn ime_target(&mut self) -> Option<ImeTarget<'_>> {
        let buffer_id = self.views.active_view()?.buffer();
        let buffer = self.workspace.buffer_mut(buffer_id)?.buffer_mut();
        let selection = self.views.active_view_mut()?.selection_mut();
        Some(ImeTarget::new(buffer, selection))
    }

    fn edit_target(&mut self) -> Option<EditTarget<'_>> {
        let buffer_id = self.views.active_view()?.buffer();
        let buffer = self.workspace.buffer_mut(buffer_id)?.buffer_mut();
        let selection = self.views.active_view_mut()?.selection_mut();
        Some(EditTarget { buffer, selection })
    }
}

impl<'a> TextTargetQuery for MainEditorOwnerRef<'a> {
    fn target_id(&self) -> TextTargetId {
        TextTargetId::MainEditor
    }

    fn is_active(&self) -> bool {
        self.views.active_view().is_some()
    }

    fn snapshot(&self) -> EditorSnapshot {
        snapshot_from_active_view(self.workspace, self.views)
    }

    fn profile(&self) -> TextInputProfile {
        TextInputProfile::MainEditor
    }

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        ime_query_from_active_view(self.workspace, self.views)
    }
}
