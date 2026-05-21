//! ShellView 焦点路由。
//!
//! 窗口里永远恰好有一个被聚焦的元素 ——「获得 / 失去焦点」只是焦点从一处移到
//! 另一处的两个视角。本模块把 actions 层的焦点移动收敛到唯一出口：workbench
//! 焦点目标的解析与 `window.focus` 调用都在这里。
//!
//! 不含 overlay：overlay 打开时捕获、关闭时归还的那枚 handle 由 overlay 流程
//! （`OverlayManager` / `dismiss_overlay`）自己保管，是另一处自洽的焦点中枢。

use gpui::{FocusHandle, Window};

use crate::shell::features::file_tree::FileTreeRuntime;
use crate::shell::features::{PanelId, PanelRuntimes, focus_panel_handle};

/// 焦点可以去的 workbench 目标。
#[derive(Clone, Copy)]
pub(super) enum FocusTarget {
    Panel(PanelId),
    Editor,
}

/// 焦点路由器：持有解析各目标所需的运行态引用，是 actions 层移动焦点的唯一出口。
pub(super) struct FocusRouter<'a> {
    panel_runtimes: &'a PanelRuntimes,
    file_tree: &'a FileTreeRuntime,
    editor: &'a FocusHandle,
}

impl<'a> FocusRouter<'a> {
    pub(super) fn new(
        panel_runtimes: &'a PanelRuntimes,
        file_tree: &'a FileTreeRuntime,
        editor: &'a FocusHandle,
    ) -> Self {
        Self {
            panel_runtimes,
            file_tree,
            editor,
        }
    }

    /// 把焦点移到目标处。骨架阶段尚无焦点宿主的 panel 静默跳过。
    pub(super) fn move_to(&self, target: FocusTarget, window: &mut Window) {
        match target {
            FocusTarget::Panel(panel) => {
                if let Some(focus) = self.panel_focus_handle(panel) {
                    // panel 可能刚被 show_panel 显示、本帧尚未布局，下一帧再聚一次。
                    focus_panel_handle(focus, window, true);
                }
            }
            FocusTarget::Editor => window.focus(self.editor),
        }
    }

    /// 焦点当前是否就在目标处。
    pub(super) fn is_at(&self, target: FocusTarget, window: &Window) -> bool {
        self.resolve(target)
            .is_some_and(|focus| focus.is_focused(window))
    }

    fn resolve(&self, target: FocusTarget) -> Option<FocusHandle> {
        match target {
            FocusTarget::Panel(panel) => self.panel_focus_handle(panel),
            FocusTarget::Editor => Some(self.editor.clone()),
        }
    }

    /// 解析 panel 的焦点宿主 handle：FileTree 的在它 runtime 上，其余在
    /// `PanelRuntimes` 里；骨架阶段尚无焦点的 panel 返回 None。
    fn panel_focus_handle(&self, panel: PanelId) -> Option<FocusHandle> {
        if panel == PanelId::FileTree {
            Some(self.file_tree.focus_handle())
        } else {
            self.panel_runtimes.focus_handle(panel)
        }
    }
}
