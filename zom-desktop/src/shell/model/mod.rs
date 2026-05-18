//! Shell 层布局模型类型（实现「桌面端布局模型.md」第 5/6 节）。
//!
//! 这些类型描述「面板有哪些 / 当前在哪个 dock 里激活 / dock 折叠了吗」，
//! 与 GPUI 视觉无关、与命令系统无关；shell 自己持有，由 app 组合根装配。
//!
//! 依赖方向：本文件只 use 标准库、gpui 几何类型与 panel 元数据，不向上 use workbench。
//!
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
    pub(crate) fn title(self) -> &'static str {
        use super::panels;
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

    /// 该 panel 在 bar 上代表的图标资源路径（embedded assets 里的相对路径）。
    /// 真理源是各 panel 模块的 `PANEL_ICON` 常量。
    pub(crate) fn icon_path(self) -> &'static str {
        use super::panels;
        match self {
            PanelId::FileTree => panels::file_tree::PANEL_ICON,
            PanelId::VersionControl => panels::version_control::PANEL_ICON,
            PanelId::Outline => panels::outline::PANEL_ICON,
            PanelId::ProjectSearch => panels::project_search::PANEL_ICON,
            PanelId::Terminal => panels::terminal::PANEL_ICON,
            PanelId::Debug => panels::debug::PANEL_ICON,
            PanelId::KeyboardShortcuts => panels::keyboard_shortcuts::PANEL_ICON,
        }
    }

    /// 切换本 panel 显隐的完整命令 id。常量本体在
    /// [`zom_command::commands::panels`]，这里只做枚举 → 常量 的映射，
    /// 供 bar glyph 等 UI 标注。
    pub(crate) fn toggle_command_id(self) -> &'static str {
        use zom_command::commands::panels as panel_cmds;
        match self {
            PanelId::FileTree => panel_cmds::TOGGLE_FILE_TREE,
            PanelId::VersionControl => panel_cmds::TOGGLE_VERSION_CONTROL,
            PanelId::Outline => panel_cmds::TOGGLE_OUTLINE,
            PanelId::ProjectSearch => panel_cmds::TOGGLE_PROJECT_SEARCH,
            PanelId::Terminal => panel_cmds::TOGGLE_TERMINAL,
            PanelId::Debug => panel_cmds::TOGGLE_DEBUG,
            PanelId::KeyboardShortcuts => panel_cmds::TOGGLE_KEYBOARD_SHORTCUTS,
        }
    }

    /// 短字符串 id —— 与 [`zom_command::HostEffect::TogglePanel`] 里
    /// String 字段对应。专门给 effect ↔ enum 之间架桥用。
    pub(crate) fn command_str_id(self) -> &'static str {
        match self {
            PanelId::FileTree => "file_tree",
            PanelId::VersionControl => "version_control",
            PanelId::Outline => "outline",
            PanelId::ProjectSearch => "project_search",
            PanelId::Terminal => "terminal",
            PanelId::Debug => "debug",
            PanelId::KeyboardShortcuts => "keyboard_shortcuts",
        }
    }

    /// 反向解析：把 `HostEffect::TogglePanel(s)` 里的字符串还原成枚举。
    pub(crate) fn from_command_str_id(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|p| p.command_str_id() == value)
    }

    /// 枚举全部 panel —— 命令注册和 bar 渲染共用。
    pub(crate) const ALL: &'static [PanelId] = &[
        PanelId::FileTree,
        PanelId::VersionControl,
        PanelId::Outline,
        PanelId::ProjectSearch,
        PanelId::Terminal,
        PanelId::Debug,
        PanelId::KeyboardShortcuts,
    ];
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
    pub(crate) editor: EditorState,
}

/// 第一版 BottomBar 渲染所需的少量动态状态（手册 17 错误呈现 / 20.8）。
#[derive(Clone, Debug, Default)]
pub(crate) struct BottomBarState {
    pub(crate) diagnostics_count: u32,
    pub(crate) lsp_connected: bool,
}

/// 主编辑区当前可显示的活动 buffer 摘要。
#[derive(Clone, Debug, Default)]
pub(crate) struct EditorState {
    pub(crate) title: String,
    pub(crate) text: String,
    pub(crate) cursor_byte: usize,
    pub(crate) dirty: bool,
}
