//! FileTree —— L3 panel：项目目录树。
//!
//! 数据来源 [`zom_workspace::ProjectTree`]，App 把它转成 owned 的
//! [`FileTreeState`] 快照传进来。面板把焦点态的按键交给宿主统一的
//! `KeyRequest`（`KeySurface::FileTree`），由 keymap 在 `FileTree` 上下文里
//! 解析 —— 面板自己不持有任何「按键 → 动作」的映射。

use std::rc::Rc;

use gpui::{AnyElement, Div, FocusHandle};

use crate::shell::editor::TextEditorSlot;
use crate::shell::workbench::PanelContext;
use crate::shell::{ActionRequest, KeyRequest};

mod confirm_delete;
mod focus;
mod model;
mod runtime;
mod state;
mod view;

pub(crate) use model::FileTreeModel;
pub(crate) use runtime::FileTreeRuntime;
pub(crate) use state::{
    FileTreeActivation, FileTreeRow, FileTreeState, PendingDelete, PendingNewEntry,
};

/// 删除确认弹窗的两个动作回调。由根视图绑定命令后注入。
pub(crate) struct ConfirmDeleteHandlers {
    pub(crate) confirm: ActionRequest,
    pub(crate) cancel: ActionRequest,
}

/// 处于删除确认态时渲染居中模态弹窗；否则返回 `None`。
///
/// 弹窗是顶层模态层，由 workbench 挂在所有面板之上。
pub(crate) fn render_confirm_delete(
    state: &FileTreeState,
    handlers: &ConfirmDeleteHandlers,
) -> Option<AnyElement> {
    state
        .pending_delete
        .as_ref()
        .map(|pending| confirm_delete::render(&pending.name, pending.kind, handlers))
}

/// 文件树面板渲染所需的所有"非状态"依赖（焦点、按键回调）。
///
/// 与 [`FileTreeState`] 区分：后者是 App 端的渲染快照，可序列化；本结构是
/// shell 端构造的回调与句柄，不进 WorkbenchState。
#[derive(Clone, Copy)]
pub(crate) struct FileTreePanel<'a> {
    pub(crate) state: &'a FileTreeState,
    pub(crate) focus: &'a FocusHandle,
    pub(crate) key_request: &'a KeyRequest,
    pub(crate) slot: &'a Rc<TextEditorSlot>,
    /// 当前焦点是否在文件树容器上；决定选中边框是否可见。
    pub(crate) is_focused: bool,
}

pub(crate) fn render(ctx: PanelContext<'_>) -> Div {
    // 无论 has_project / 是否有行，都走 view::render —— 它负责挂 track_focus
    // 与 on_key_down，避免"焦点宿主在某些状态下从树里消失"的坑。占位文本
    // 由 view 自己挑。
    view::render(ctx)
}
