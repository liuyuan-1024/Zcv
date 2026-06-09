//! 主编辑区文本目标 owner。
//!
//! 把 `Workspace + ViewSet` 适配成 [`TextTargetOwner`] / [`TextTargetQuery`]，
//! 让 [`crate::text_target::EditorRouter`] 能像对待小输入框一样统一路由主编辑区。

use zom_command::{EditTarget, KeyContext};
use zom_view::{ViewId, ViewSet, ViewportState, WrapMap};
use zom_workspace::Workspace;

use crate::editor::highlight;
use crate::editor::text::{
    EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, ImeTarget, RevealHint, build_snapshot,
};
use crate::focus::AppFocus;
use crate::text_target::{TextTargetOwner, TextTargetQuery};

/// 写入侧：路由要 `&mut` 时构造它。
///
/// `active_view_id` 由 [`crate::workspace_session::WorkspaceSession::active_view_id`] 在构造前快照得到——main owner 自己不再回 `ViewSet` 查活动 view。
pub(crate) struct MainEditorOwner<'a> {
    workspace: &'a mut Workspace,
    views: &'a mut ViewSet,
    active_view_id: Option<ViewId>,
}

/// 只读侧：路由非可变路径时构造它。
pub(crate) struct MainEditorOwnerRef<'a> {
    workspace: &'a Workspace,
    views: &'a ViewSet,
    active_view_id: Option<ViewId>,
}

impl<'a> MainEditorOwner<'a> {
    pub(crate) fn new(
        workspace: &'a mut Workspace,
        views: &'a mut ViewSet,
        active_view_id: Option<ViewId>,
    ) -> Self {
        Self {
            workspace,
            views,
            active_view_id,
        }
    }
}

impl<'a> MainEditorOwnerRef<'a> {
    pub(crate) fn new(
        workspace: &'a Workspace,
        views: &'a ViewSet,
        active_view_id: Option<ViewId>,
    ) -> Self {
        Self {
            workspace,
            views,
            active_view_id,
        }
    }
}

fn settle_active_view_y(
    workspace: &Workspace,
    views: &mut ViewSet,
    active_view_id: Option<ViewId>,
) {
    let Some(view) = active_view_id.and_then(|id| views.edit_view_mut(id)) else {
        return;
    };
    let Some(buffer) = workspace.buffer(view.buffer()) else {
        return;
    };
    let buf = buffer.buffer();
    view.settle_viewport_y(buf, view.selection().primary().head());
}

/// 算出 snapshot 应当切出的逻辑行范围 `(start, len)`。
///
/// 算法：以视觉行坐标为准，在视口窗口的两侧各留 `visible_visual_rows` 行作为冗余（共 3 个视口高度），
/// 再用 [`WrapMap::visual_row_to_line_subrow`] 把视觉行端点映回逻辑行。
/// 这样软换行严重时（一条逻辑行覆盖远多于一个视口）也能保证视口顶 / 底所属的整条逻辑行都进得了 snapshot。
///
/// 无 `wrap_map` 时按逻辑行切片；测量完成后再用视觉行反查精确范围。
fn compute_snapshot_logical_slice(vp: ViewportState, wrap_map: Option<&WrapMap>) -> (u64, u64) {
    let visible = vp.visible_visual_rows.max(1);
    let (slice_start_line, slice_end_line) = match wrap_map {
        Some(wm) => {
            let top = wm.visual_row_of(vp.top_line, vp.top_subrow as u32);
            let head = top.saturating_sub(visible);
            let tail = top.saturating_add(visible).saturating_add(visible);
            let (start_line, _) = wm.visual_row_to_line_subrow(head);
            let (end_line, _) = wm.visual_row_to_line_subrow(tail);
            (start_line, end_line)
        }
        None => {
            // 无 wrap_map：top_visual == top_line，逻辑行号即视觉行号。
            let head = vp.top_line.saturating_sub(visible);
            let tail = vp.top_line.saturating_add(visible).saturating_add(visible);
            (head, tail)
        }
    };
    let len = slice_end_line
        .saturating_sub(slice_start_line)
        .saturating_add(1);
    (slice_start_line, len)
}

fn snapshot_from_active_view(
    workspace: &Workspace,
    views: &ViewSet,
    active_view_id: Option<ViewId>,
) -> EditorSnapshot {
    let Some(view) = active_view_id.and_then(|id| views.edit_view(id)) else {
        return EditorSnapshot::default();
    };
    let Some(buffer) = workspace.buffer(view.buffer()) else {
        return EditorSnapshot::default();
    };
    let selection = view.selection().clone();

    let vp = view.viewport();
    let (slice_start, slice_len) = compute_snapshot_logical_slice(vp, view.wrap_map());
    let request = EditorSnapshotRequest::viewport(slice_start, slice_len);
    let mut snapshot = build_snapshot(buffer.buffer(), &selection, request);
    snapshot.top_line = vp.top_line;
    snapshot.top_subrow = vp.top_subrow;
    snapshot.visual_caret = view.visual_caret().copied();

    let reveal = view.reveal().and_then(|req| {
        let line = buffer.buffer().byte_to_position(req.byte).ok()?;
        Some(RevealHint {
            byte: req.byte.get(),
            line: line.line().get() as u64,
            kind: req.kind,
            seq: req.seq,
        })
    });
    snapshot.reveal = reveal;
    highlight::push_workspace_search(buffer, &mut snapshot.decorations);
    // syntax decoration 从共享 BufferSyntaxTree 现查 viewport-scoped Query。
    // `slot.load()` 返回 `Arc<BufferSyntaxTree>` clone（计数 +1，无锁路径）；
    // `as_deref` 把 `Option<Arc<_>>` 借成 `Option<&BufferSyntaxTree>` 给 push 用。
    let syntax_tree = buffer.syntax_tree_slot().and_then(|slot| slot.load());
    highlight::push_syntax_tree(
        syntax_tree.as_deref(),
        &snapshot.lines,
        &mut snapshot.decorations,
    );
    snapshot
}

fn ime_query_from_active_view<'a>(
    workspace: &'a Workspace,
    views: &'a ViewSet,
    active_view_id: Option<ViewId>,
) -> Option<ImeQueryTarget<'a>> {
    let view = active_view_id.and_then(|id| views.edit_view(id))?;
    let buffer = workspace.buffer(view.buffer())?.buffer();
    Some(ImeQueryTarget::new(buffer, view.selection()))
}

impl<'a> TextTargetQuery for MainEditorOwner<'a> {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        matches!(focus, AppFocus::Editor(_))
    }

    fn snapshot(&self, _focus: AppFocus) -> EditorSnapshot {
        snapshot_from_active_view(self.workspace, self.views, self.active_view_id)
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        vec![
            KeyContext::text_edit(self.accepts_newline(), false),
            KeyContext::global(),
        ]
    }

    fn accepts_newline(&self) -> bool {
        true
    }

    fn ime_query_target(&self, _focus: AppFocus) -> Option<ImeQueryTarget<'_>> {
        ime_query_from_active_view(self.workspace, self.views, self.active_view_id)
    }
}

impl<'a> TextTargetOwner for MainEditorOwner<'a> {
    fn ime_target(&mut self, _focus: AppFocus) -> Option<ImeTarget<'_>> {
        let view_id = self.active_view_id?;
        let buffer_id = self.views.edit_view(view_id)?.buffer();
        let buffer = self.workspace.buffer_mut(buffer_id)?.buffer_mut();
        let selection = self.views.edit_view_mut(view_id)?.selection_mut();
        Some(ImeTarget::new(buffer, selection))
    }

    fn edit_target(&mut self, _focus: AppFocus) -> Option<EditTarget<'_>> {
        let view_id = self.active_view_id?;
        let buffer_id = self.views.edit_view(view_id)?.buffer();
        let buffer = self.workspace.buffer_mut(buffer_id)?.buffer_mut();
        let view = self.views.edit_view_mut(view_id)?;
        let (selection, visual_caret, goal_column, wrap_map) = view.vertical_movement_state_mut();
        Some(EditTarget {
            buffer,
            selection,
            wrap_map,
            visual_caret: Some(visual_caret),
            goal_column: Some(goal_column),
        })
    }

    fn settle_viewport_y(&mut self) {
        settle_active_view_y(self.workspace, self.views, self.active_view_id);
    }
}

impl<'a> TextTargetQuery for MainEditorOwnerRef<'a> {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        matches!(focus, AppFocus::Editor(_))
    }

    fn snapshot(&self, _focus: AppFocus) -> EditorSnapshot {
        snapshot_from_active_view(self.workspace, self.views, self.active_view_id)
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        vec![
            KeyContext::text_edit(self.accepts_newline(), false),
            KeyContext::global(),
        ]
    }

    fn accepts_newline(&self) -> bool {
        true
    }

    fn ime_query_target(&self, _focus: AppFocus) -> Option<ImeQueryTarget<'_>> {
        ime_query_from_active_view(self.workspace, self.views, self.active_view_id)
    }
}
