//! Panel 功能身份与元数据映射。
//!
//! 这里维护 desktop 第一版固定 panel 列表。具体 UI 仍在各功能目录内，`PanelId`
//! 只负责把命令、布局与功能模块连接起来。

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
        use super::{
            debug, file_tree, keyboard_shortcuts, outline, project_search, terminal,
            version_control,
        };
        match self {
            PanelId::FileTree => file_tree::panel_title(),
            PanelId::VersionControl => version_control::panel_title(),
            PanelId::Outline => outline::panel_title(),
            PanelId::ProjectSearch => project_search::panel_title(),
            PanelId::Terminal => terminal::panel_title(),
            PanelId::Debug => debug::panel_title(),
            PanelId::KeyboardShortcuts => keyboard_shortcuts::panel_title(),
        }
    }

    /// 该 panel 在 bar 上代表的图标资源路径（embedded assets 里的相对路径）。
    /// 真理源是各 panel 模块的 `PANEL_ICON` 常量。
    pub(crate) fn icon_path(self) -> &'static str {
        use super::{
            debug, file_tree, keyboard_shortcuts, outline, project_search, terminal,
            version_control,
        };
        match self {
            PanelId::FileTree => file_tree::PANEL_ICON,
            PanelId::VersionControl => version_control::PANEL_ICON,
            PanelId::Outline => outline::PANEL_ICON,
            PanelId::ProjectSearch => project_search::PANEL_ICON,
            PanelId::Terminal => terminal::PANEL_ICON,
            PanelId::Debug => debug::PANEL_ICON,
            PanelId::KeyboardShortcuts => keyboard_shortcuts::PANEL_ICON,
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
            .find(|panel| panel.command_str_id() == value)
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
