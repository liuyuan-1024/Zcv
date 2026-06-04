//! 文本目标路由协议。
//!
//! 这是 app 与 shell 之间的共享词汇：
//! app 只知道“某个文本目标 owner 能按 [`crate::focus::AppFocus`] 提供命令 / IME 路由能力”，
//! 不知道这些 owner 来自哪个面板、surface 或 GPUI 组件。

use std::cell::{Ref, RefCell, RefMut};
use std::ops::Range;
use std::rc::Rc;

use zom_command::{CommandError, EditTarget, KeyContext};
use zom_view::ViewSet;
use zom_workspace::Workspace;

use crate::focus::AppFocus;
use crate::shell::editor::{
    self, EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, ImeTarget, RevealHint,
    build_snapshot,
};

/// 只读侧：是哪个 target、当前是否活跃、给路由用的查询能力。
pub(crate) trait TextTargetQuery {
    /// 这个 owner 是否承载指定的应用语义焦点。
    fn accepts_focus(&self, focus: AppFocus) -> bool;

    /// 给指定 focus 的快照。多 focus owner（search）按 focus 选字段；单 focus
    /// owner 忽略参数。
    fn snapshot(&self, focus: AppFocus) -> EditorSnapshot;

    /// 该 owner 聚焦时的按键解析上下文栈（优先级从高到低）。
    fn key_contexts(&self) -> Vec<KeyContext>;

    /// 该 owner 在文本编辑层是否接受换行（影响 `KeyContext::text_edit` 的参数）。
    fn accepts_newline(&self) -> bool {
        false
    }

    fn ime_query_target(&self, focus: AppFocus) -> Option<ImeQueryTarget<'_>>;
}

/// 可写侧：IME 写入与编辑命令作用目标。
pub(crate) trait TextTargetOwner: TextTargetQuery {
    fn ime_target(&mut self, focus: AppFocus) -> Option<ImeTarget<'_>>;
    fn edit_target(&mut self, focus: AppFocus) -> Option<EditTarget<'_>>;

    /// 文本输入后置钩子。默认无操作；owner 想响应“文本变了”时自行 override。
    fn after_text_changed(&mut self) {}

    /// 视口 Y 轴落定钩子。默认无操作。
    fn settle_viewport_y(&mut self) {}
}

/// 只读路由：current_focus key contexts / snapshot / preedit 等。
pub(crate) struct EditorRouter<'a> {
    owners: Vec<&'a dyn TextTargetQuery>,
}

/// 可写路由：仅承载 IME 写入回调（不返回借用）。
pub(crate) struct EditorRouterMut<'a> {
    owners: Vec<&'a mut dyn TextTargetOwner>,
}

impl<'a> EditorRouter<'a> {
    pub(crate) fn new(owners: Vec<&'a dyn TextTargetQuery>) -> Self {
        Self { owners }
    }

    pub(crate) fn key_contexts_for(&self, focus: AppFocus) -> Option<Vec<KeyContext>> {
        self.owners
            .iter()
            .find(|owner| owner.accepts_focus(focus))
            .map(|owner| owner.key_contexts())
    }

    pub(crate) fn is_composing(&self, focus: AppFocus) -> bool {
        self.preedit_text(focus)
            .is_some_and(|preedit| !preedit.is_empty())
    }

    pub(crate) fn snapshot_for_focus(&self, focus: AppFocus) -> EditorSnapshot {
        self.owners
            .iter()
            .find(|owner| owner.accepts_focus(focus))
            .map(|owner| owner.snapshot(focus))
            .unwrap_or_default()
    }

    pub(crate) fn marked_range_utf16(&self, focus: AppFocus) -> Option<Range<usize>> {
        self.with_query(focus, |q| q.marked_range_utf16()).flatten()
    }

    pub(crate) fn selected_range_utf16(&self, focus: AppFocus) -> Option<(Range<usize>, bool)> {
        self.with_query(focus, |q| q.selected_range_utf16())
    }

    pub(crate) fn text_for_range_utf16(
        &self,
        focus: AppFocus,
        range: Range<usize>,
    ) -> Option<String> {
        self.with_query(focus, |q| q.text_for_range_utf16(range))
            .flatten()
    }

    pub(crate) fn preedit_text(&self, focus: AppFocus) -> Option<String> {
        self.with_query(focus, |q| q.preedit_text()).flatten()
    }

    fn with_query<R>(
        &self,
        focus: AppFocus,
        f: impl FnOnce(&ImeQueryTarget<'_>) -> R,
    ) -> Option<R> {
        let owner = self
            .owners
            .iter()
            .find(|owner| owner.accepts_focus(focus))?;
        let query = owner.ime_query_target(focus)?;
        Some(f(&query))
    }
}

impl<'a> EditorRouterMut<'a> {
    pub(crate) fn new(owners: Vec<&'a mut dyn TextTargetOwner>) -> Self {
        Self { owners }
    }

    pub(crate) fn settle_viewport_for_focus(&mut self, focus: AppFocus) {
        if let Some(owner) = self.owners.iter_mut().find(|o| o.accepts_focus(focus)) {
            owner.settle_viewport_y();
        }
    }

    pub(crate) fn with_ime_target<R>(
        mut self,
        focus: AppFocus,
        f: impl FnOnce(ImeTarget<'_>) -> Result<R, CommandError>,
    ) -> Result<R, CommandError> {
        for owner in self.owners.iter_mut() {
            if owner.accepts_focus(focus) {
                let ime = owner.ime_target(focus).ok_or(CommandError::NoActiveView)?;
                let result = f(ime)?;
                owner.after_text_changed();
                return Ok(result);
            }
        }
        Err(CommandError::NoActiveView)
    }
}

/// 外部 owner 注册表。shell runtime 通过 `App::install_editor_owner` 注册 owner。
#[derive(Default)]
pub(crate) struct EditorTargetRegistry {
    owners: Vec<Rc<RefCell<dyn TextTargetOwner>>>,
}

impl EditorTargetRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register(&mut self, owner: Rc<RefCell<dyn TextTargetOwner>>) {
        self.owners.push(owner);
    }

    pub(crate) fn borrow_all(&self) -> Vec<Ref<'_, dyn TextTargetOwner>> {
        let mut out = Vec::with_capacity(self.owners.len());
        for rc in &self.owners {
            out.push(rc.borrow());
        }
        out
    }

    pub(crate) fn borrow_all_mut(&self) -> Vec<RefMut<'_, dyn TextTargetOwner + 'static>> {
        let mut out = Vec::with_capacity(self.owners.len());
        for rc in &self.owners {
            out.push(rc.borrow_mut());
        }
        out
    }
}

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
    editor::highlight::push_workspace_search(buffer, &mut snapshot.decorations);
    editor::highlight::push_syntax_layers(
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
