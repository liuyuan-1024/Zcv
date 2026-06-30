//! Workbench 布局运行态（实现「桌面端布局模型.md」第 5/6 节）。
//!
//! 这些类型只描述窗口级布局：当前有哪些 panel、各 dock 是否折叠、bottom bar 状态等。
//! 各 feature（编辑区 / 文件树 / 搜索）的视图快照不进 [`WorkbenchState`]，由 view 装配层在渲染瞬间各自构造，
//! 旁路传给 [`PanelContext`] / `editor_area::render` / `bottom_bar::render`。
//!
//! [`PanelContext`]: super::PanelContext

use gpui::Pixels;

use crate::ui_id::PanelId;

/// 三种停靠区域（布局模型 5）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DockAreaId {
    Left,
    Right,
    /// `CenterColumn` 下方的 `BottomDock`，不是窗口级 `BottomBar`。
    Bottom,
}

/// 当前单栈面板模型（布局模型 5.3 / 手册 20.4）。
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
        self.panels.contains(&panel)
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
    pub(crate) fn new(stack: PanelStack, default_size: Pixels) -> Self {
        Self {
            collapsed: true,
            size: default_size,
            stack,
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        !self.collapsed && self.stack.active().is_some()
    }

    pub(crate) fn active_panel(&self) -> Option<PanelId> {
        self.stack.active()
    }
}

/// 窗口级 workbench 的布局状态（手册 13.2 表："每窗口独立"列）。
///
/// 只描述 chrome / dock 视觉：feature panel 的内容由 feature 自己向 view 装配层提供，不进本结构。
/// Workbench 负责"哪里显示"，feature 负责"显示什么"。
#[derive(Clone, Debug)]
pub(crate) struct WorkbenchState {
    pub(crate) project_title: String,
    /// 当前项目所在 git 仓库的分支名（`git rev-parse --abbrev-ref HEAD`）。
    /// 仅在项目是 git 仓库且 HEAD 指向分支时存在。
    pub(crate) project_branch: Option<String>,
    pub(crate) has_project: bool,
    pub(crate) left_dock: DockState,
    pub(crate) right_dock: DockState,
    pub(crate) bottom_dock: DockState,
    pub(crate) bottom_bar: BottomBarState,
}

/// BottomBar 渲染所需的少量动态状态（手册 17 错误呈现 / 20.8）。
#[derive(Clone, Debug, Default)]
pub(crate) struct BottomBarState {
    pub(crate) diagnostics_count: u32,
    pub(crate) lsp_connected: bool,
}
