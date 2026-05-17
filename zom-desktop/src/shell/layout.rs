//! Shell 层布局模型类型（实现「桌面端布局模型.md」第 5/6 节）。
//!
//! 这些类型描述「面板有哪些 / 当前在哪个 dock 里激活 / dock 折叠了吗」，
//! 与 GPUI 视觉无关、与命令系统无关；shell 自己持有，由 app 组合根装配。
//!
//! 依赖方向：本文件只 use 标准库与 gpui 几何类型，不向上 use components。
//!
//! 骨架阶段：`PanelId::as_str`、`PanelStack::panels`、`DockState::area`
//! 等成员是「即将被命令注册 / 持久化 / 拖动重排消费」的稳定 API，先抑制
//! dead_code 警告。

#![allow(dead_code)]

use gpui::Pixels;

/// 桌面端第一版固定的 panel 列表（手册 20.10）。
///
/// 不抽 `PanelProvider` trait（手册 20.2）；新增 panel 直接在此 enum 加变体。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum PanelId {
    FileTree,
    VersionControl,
    Outline,
    ProjectSearch,
    Terminal,
    Debug,
    KeyboardShortcuts,
}

impl PanelId {
    /// 与各 panel 模块的 `PANEL_ID` 常量保持同步——只在显示 / 日志 / 命令
    /// 名查找时使用，不参与渲染分支判定。
    pub(crate) fn as_str(self) -> &'static str {
        use super::components::panels;
        match self {
            PanelId::FileTree => panels::file_tree::PANEL_ID,
            PanelId::VersionControl => panels::version_control::PANEL_ID,
            PanelId::Outline => panels::outline::PANEL_ID,
            PanelId::ProjectSearch => panels::project_search::PANEL_ID,
            PanelId::Terminal => panels::terminal::PANEL_ID,
            PanelId::Debug => panels::debug::PANEL_ID,
            PanelId::KeyboardShortcuts => panels::keyboard_shortcuts::PANEL_ID,
        }
    }

    pub(crate) fn title(self) -> &'static str {
        use super::components::panels;
        match self {
            PanelId::FileTree => panels::file_tree::panel_title(),
            PanelId::VersionControl => panels::version_control::panel_title(),
            PanelId::Outline => panels::outline::panel_title(),
            PanelId::ProjectSearch => panels::project_search::panel_title(),
            PanelId::Terminal => panels::terminal::panel_title(),
            PanelId::Debug => panels::debug::panel_title(),
            PanelId::KeyboardShortcuts => panels::keyboard_shortcuts::panel_title(),
        }
    }
}

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
    pub(crate) area: DockAreaId,
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
    pub(crate) left_dock: DockState,
    pub(crate) right_dock: DockState,
    pub(crate) bottom_dock: DockState,
    pub(crate) bottom_bar: BottomBarState,
}

/// 第一版 BottomBar 渲染所需的少量动态状态（手册 17 错误呈现 / 20.8）。
#[derive(Clone, Debug, Default)]
pub(crate) struct BottomBarState {
    pub(crate) diagnostics_count: u32,
    pub(crate) lsp_connected: bool,
}
