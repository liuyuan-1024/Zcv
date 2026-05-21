//! FileTree —— L3 panel：项目目录树。
//!
//! 数据来源 [`zom_workspace::ProjectTree`]，App 把它转成 owned 的
//! [`FileTreeState`] 快照传进来。本面板自己接收
//! 焦点并处理 ↑/↓/←/→/Enter/Esc；不命中的键交给 panel-context 携带的全局
//! `KeyRequest` 走 keymap 管线。

use std::rc::Rc;

use gpui::{App as GpuiApp, Div, FocusHandle, Keystroke, Window};

use crate::shell::InputHandlerHook;
use crate::shell::workbench::PanelContext;

mod focus;
mod keyboard;
mod model;
mod runtime;
mod state;
mod view;

pub(crate) const PANEL_ICON: &str = "icons/bottom_bar/file_tree.svg";

pub(crate) use model::FileTreeModel;
pub(crate) use runtime::FileTreeRuntime;
pub(crate) use state::{FileTreeActivation, FileTreeRow, FileTreeState, PendingNewEntry};

/// 文件树面板内部按键处理回调。
///
/// 入参是原始 [`Keystroke`]（新建条目输入态需要 `key_char`）；返回 `true`
/// 表示已消费该按键。
pub(crate) type FileTreeKeyRequest = Rc<dyn Fn(&Keystroke, &mut Window, &mut GpuiApp) -> bool>;

/// 文件树面板渲染所需的所有"非状态"依赖（焦点、按键回调）。
///
/// 与 [`FileTreeState`] 区分：后者是 App 端的渲染快照，可序列化；本结构是
/// shell 端构造的回调与句柄，不进 WorkbenchState。
#[derive(Clone, Copy)]
pub(crate) struct FileTreePanel<'a> {
    pub(crate) state: &'a FileTreeState,
    pub(crate) focus: &'a FocusHandle,
    pub(crate) key_request: &'a FileTreeKeyRequest,
    pub(crate) input_handler_hook: &'a InputHandlerHook,
    /// 当前焦点是否在文件树容器上；决定选中边框是否可见。
    pub(crate) is_focused: bool,
}

pub(crate) fn render(ctx: PanelContext<'_>) -> Div {
    // 无论 has_project / 是否有行，都走 view::render —— 它负责挂 track_focus
    // 与 on_key_down，避免"焦点宿主在某些状态下从树里消失"的坑。占位文本
    // 由 view 自己挑。
    view::render(ctx)
}
