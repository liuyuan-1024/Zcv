//! 主编辑区文本目标 owner。
//!
//! 把 `Workspace + ViewSet` 适配成 [`TextTargetOwner`] / [`TextTargetQuery`]，
//! 让 [`crate::text_target::EditorRouter`] 能像对待小输入框一样统一路由主编辑区。

use zom_command::{EditTarget, KeyContext};
use zom_view::ViewSet;
use zom_workspace::Workspace;

use crate::editor::highlight;
use crate::editor::text::{
    EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, ImeTarget, RevealHint, build_snapshot,
};
use crate::focus::AppFocus;
use crate::text_target::{TextTargetOwner, TextTargetQuery};

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

fn settle_active_view_y(workspace: &Workspace, views: &mut ViewSet) {
    let Some(view) = views.active_view_mut() else {
        return;
    };
    let Some(buffer) = workspace.buffer(view.buffer()) else {
        return;
    };
    let total_lines = buffer.buffer().line_count() as u64;
    let selection_head_line = buffer
        .buffer()
        .byte_to_position(view.selection().primary().head())
        .map(|pos| pos.line().get() as u64)
        .unwrap_or(0);
    let reveal_line = view.reveal().and_then(|req| {
        buffer
            .buffer()
            .byte_to_position(req.byte)
            .ok()
            .map(|pos| pos.line().get() as u64)
    });
    view.settle_viewport_y(total_lines, selection_head_line, reveal_line);
}

fn snapshot_from_active_view(workspace: &Workspace, views: &ViewSet) -> EditorSnapshot {
    let Some(view) = views.active_view() else {
        return EditorSnapshot::default();
    };
    let Some(buffer) = workspace.buffer(view.buffer()) else {
        return EditorSnapshot::default();
    };
    let selection = view.selection().clone();

    let vp = view.viewport();
    let visible_lines = vp.visible_logical_lines;
    let slice_start = vp.top_line.saturating_sub(visible_lines);
    let slice_len = visible_lines.saturating_mul(3);
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
    highlight::push_syntax_layers(
        buffer.highlight_layers(),
        &snapshot.lines,
        &mut snapshot.decorations,
    );
    snapshot
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
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        matches!(focus, AppFocus::Editor(_))
    }

    fn snapshot(&self, _focus: AppFocus) -> EditorSnapshot {
        snapshot_from_active_view(self.workspace, self.views)
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
        ime_query_from_active_view(self.workspace, self.views)
    }
}

impl<'a> TextTargetOwner for MainEditorOwner<'a> {
    fn ime_target(&mut self, _focus: AppFocus) -> Option<ImeTarget<'_>> {
        let buffer_id = self.views.active_view()?.buffer();
        let buffer = self.workspace.buffer_mut(buffer_id)?.buffer_mut();
        let selection = self.views.active_view_mut()?.selection_mut();
        Some(ImeTarget::new(buffer, selection))
    }

    fn edit_target(&mut self, _focus: AppFocus) -> Option<EditTarget<'_>> {
        let buffer_id = self.views.active_view()?.buffer();
        let buffer = self.workspace.buffer_mut(buffer_id)?.buffer_mut();
        let view = self.views.active_view_mut()?;
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
        settle_active_view_y(self.workspace, self.views);
    }
}

impl<'a> TextTargetQuery for MainEditorOwnerRef<'a> {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        matches!(focus, AppFocus::Editor(_))
    }

    fn snapshot(&self, _focus: AppFocus) -> EditorSnapshot {
        snapshot_from_active_view(self.workspace, self.views)
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
        ime_query_from_active_view(self.workspace, self.views)
    }
}
