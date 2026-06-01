//! 主编辑区的"虚拟"文本目标 owner —— 把 `Workspace + ViewSet` 适配到
//! [`TextTargetOwner`] / [`TextTargetQuery`]。
//!
//! 主编辑区不像文件树 / 项目选择器那样在 feature 模型里持有独立自持文本目标
//! —— 它的 buffer 在 [`Workspace`] 里、selection 在 [`ViewSet`] 的活动视图里。
//! 路由要求统一通过 [`TextTargetOwner`] / [`TextTargetQuery`] 反查，所以这里
//! 包一个透视结构：每次方法调用都从 workspace/view 当场拼出 ImeTarget /
//! EditTarget / 快照。
//!
//! 读 / 写两个变体分别绑不同的借用强度 —— 路由的查询路径只持 `&workspace +
//! &views`，写入路径才升级到 `&mut`。
//!
//! 之所以住在 editor_area 而不是 `shell/editor/`：本结构反向依赖
//! `zom_workspace` / `zom_view`，而编辑器子系统应该对调用方一无所知。
//! 这里是主编辑区把自己"翻译"成编辑器子系统能理解的 owner 形态的胶水层。

use zom_command::{EditTarget, KeyContext};
use zom_view::ViewSet;
use zom_workspace::Workspace;

use crate::focus::AppFocus;
use crate::shell::editor::highlight::producers;
use crate::shell::editor::{
    EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, ImeTarget, RevealHint, TextTargetOwner,
    TextTargetQuery, build_snapshot,
};

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

/// 在 build_snapshot 之前把 view.viewport.top_line 推进到本帧应该切的窗口。
///
/// 主编辑区 owner 在 `TextTargetOwner::settle_viewport_y` 钩子里调本函数；
/// 在 `slot::embed()` 的渲染入口处由 router 统一触发，保证每帧 snapshot 拿到的视口已经吸收完 pending reveal 与 edge-scroll，
/// 不再依赖"上一帧 prepaint 测出来的 top_line"——光标远跳就不会产生 1 帧空白。
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

    // 视口边界来自 view —— `View::new` 用 `DEFAULT_INITIAL_VISIBLE_LINES` 初始化，
    // element 在 prepaint 末尾按 bounds / line_height 测量后 sync 回写真实值。
    let vp = view.viewport();
    let visible_lines = vp.visible_line_count;
    // 上下各加 visible_lines：让 edge-scroll 把视口推出原范围时（PageDown / 连按方向键）本帧 lines 已经覆盖新可见行，避免 1 帧空窗。
    // `viewport_start_line` 在快照里反映的是实际切片起点，element 用它算 top_adjusted。
    let slice_start = vp.top_line.saturating_sub(visible_lines);
    let slice_len = visible_lines.saturating_mul(3);
    let request = EditorSnapshotRequest::viewport(slice_start, slice_len);
    let mut snapshot = build_snapshot(buffer.buffer(), &selection, request);
    // view 已落定的 top_line（真实视口起点）；与 slice_start 区分开供 element 直接用。
    snapshot.top_line = vp.top_line;

    // reveal 携带的 byte 要折一次 byte_to_position 出逻辑行——element 看不到 buffer。
    // 离开视口的 reveal 目标全靠这条 line 决定怎么滚。
    // 失败时丢掉 reveal（视为已过期），不让一个坏 byte 卡住渲染。
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
    // 主编辑区独有的两个 producer：search 与 syntax。
    // selection 已在 [`build_snapshot`] 内的 [`producers::selection`] 路径产出，单行输入框也复用那一路径。
    // 主编辑区不需要再加一遍。
    producers::search::push(buffer, &mut snapshot.decorations);
    producers::syntax::push(buffer, &snapshot.lines, &mut snapshot.decorations);
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

    fn snapshot(&self) -> EditorSnapshot {
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

    fn settle_viewport_y(&mut self) {
        settle_active_view_y(self.workspace, self.views);
    }
}

impl<'a> TextTargetQuery for MainEditorOwnerRef<'a> {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        matches!(focus, AppFocus::Editor(_))
    }

    fn snapshot(&self) -> EditorSnapshot {
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

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        ime_query_from_active_view(self.workspace, self.views)
    }
}
