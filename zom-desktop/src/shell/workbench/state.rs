//! Workbench 布局运行态（实现「桌面端布局模型.md」第 5/6 节）。
//!
//! 这些类型描述窗口级布局：当前有哪些 panel、各 dock 是否折叠、编辑区摘要等。

use gpui::Pixels;

pub(crate) use crate::app::{EditorState, EditorTab};
use crate::shell::features::panels::PanelId;
use crate::shell::features::panels::file_tree::FileTreeState;

/// 三种停靠区域（布局模型 5）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DockAreaId {
    Left,
    Right,
    /// `CenterColumn` 下方的 `BottomDock`，不是窗口级 `BottomBar`。
    Bottom,
}

/// 第一版单栈面板模型（布局模型 5.3 / 手册 20.4）。
///
/// 同一时间一个 dock 内最多显示 1 个 panel；切换 active 即切换显示。
#[derive(Clone, Debug)]
pub(crate) struct PanelStack {
    pub(crate) panels: Vec<PanelId>,
    pub(crate) active: Option<PanelId>,
}

impl PanelStack {
    pub(crate) fn new(panels: Vec<PanelId>, active: Option<PanelId>) -> Self {
        Self { panels, active }
    }

    pub(crate) fn active(&self) -> Option<PanelId> {
        self.active
    }

    /// 该 stack 是否承载某个 panel（用于 BottomBar 决定槽的归属 dock）。
    pub(crate) fn contains(&self, panel: PanelId) -> bool {
        self.panels.iter().any(|p| *p == panel)
    }
}

/// 单个 Dock 的运行时状态（手册 20.6）。
#[derive(Clone, Debug)]
pub(crate) struct DockState {
    pub(crate) collapsed: bool,
    pub(crate) size: Pixels,
    pub(crate) stack: PanelStack,
}

impl DockState {
    pub(crate) fn is_visible(&self) -> bool {
        !self.collapsed && self.stack.active().is_some()
    }

    pub(crate) fn active_panel(&self) -> Option<PanelId> {
        self.stack.active()
    }
}

/// 窗口级 workbench 的全部布局状态（手册 13.2 表："每窗口独立"列）。
#[derive(Clone, Debug)]
pub(crate) struct WorkbenchState {
    pub(crate) project_title: String,
    pub(crate) has_project: bool,
    pub(crate) left_dock: DockState,
    pub(crate) right_dock: DockState,
    pub(crate) bottom_dock: DockState,
    pub(crate) bottom_bar: BottomBarState,
    pub(crate) editor: EditorState,
    pub(crate) file_tree: FileTreeState,
}

/// 第一版 BottomBar 渲染所需的少量动态状态（手册 17 错误呈现 / 20.8）。
#[derive(Clone, Debug, Default)]
pub(crate) struct BottomBarState {
    pub(crate) diagnostics_count: u32,
    pub(crate) lsp_connected: bool,
}
