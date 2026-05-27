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
use crate::shell::editor::{
    EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, ImeTarget, RevealHint, TextTargetOwner,
    TextTargetQuery, build_snapshot,
};

/// 视口尚未由 element 反算写回时（即 `ViewportState.visible_line_count == 0`，
/// 例如 app 启动后第一帧 / headless 测试）退而求其次取的可见行数。
///
/// 选个比常见屏幕略大的值：4K + 14px 行高 ~ 280 行；200 是兼顾"够大屏首屏看
/// 全"和"小 buffer 别 over-allocate"的折中。第二帧起会被 element 写回值覆盖。
const DEFAULT_VISIBLE_LINES: u64 = 200;

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

    // 视口边界来自 view —— element 在前一帧 prepaint 末尾按 bounds / line_height
    // 反算并写回。首帧 / headless 测试场景下 `visible_line_count == 0`，退到
    // `DEFAULT_VISIBLE_LINES`，保证首屏不会少读行（也不会读整个 10G 文件）。
    let vp = view.viewport();
    let visible_lines = if vp.visible_line_count == 0 {
        DEFAULT_VISIBLE_LINES
    } else {
        vp.visible_line_count
    };
    let request = EditorSnapshotRequest::viewport(vp.top_line, visible_lines);
    let mut snapshot = build_snapshot(buffer.buffer(), &selection, request);

    // reveal 携带的 byte 要折一次 byte_to_position 出逻辑行——element 看不到
    // buffer，离开视口的 reveal 目标全靠这条 line 决定怎么滚。失败时丢掉
    // reveal（视为已过期），不让一个坏 byte 卡住渲染。
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
    snapshot.search_hits = search_hits;
    snapshot.search_current = search_current;
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
