//! Panel 功能身份与元数据。
//!
//! 这里维护 desktop 第一版固定 panel 列表。具体 UI 仍在各功能目录内，`PanelId`
//! 负责把命令、布局与功能模块连接起来。承载小件（焦点宿主、骨架占位）属于
//! workbench 的 panel 框架，见 `workbench::docks`。

use gpui::{FocusHandle, Window};

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

    /// 切换本 panel 显隐的完整命令 id。常量本体在各自
    /// `zom_command::commands::<feature>` 模块，这里只做枚举 → 常量 的映射，
    /// 供 bar glyph 等 UI 标注。
    pub(crate) fn toggle_command_id(self) -> &'static str {
        use zom_command::commands::{
            debug, file_tree, keyboard_shortcuts, outline, project_search, terminal,
            version_control,
        };
        match self {
            PanelId::FileTree => file_tree::TOGGLE_PANEL,
            PanelId::VersionControl => version_control::TOGGLE_PANEL,
            PanelId::Outline => outline::TOGGLE_PANEL,
            PanelId::ProjectSearch => project_search::TOGGLE_PANEL,
            PanelId::Terminal => terminal::TOGGLE_PANEL,
            PanelId::Debug => debug::TOGGLE_PANEL,
            PanelId::KeyboardShortcuts => keyboard_shortcuts::TOGGLE_PANEL,
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

pub(crate) fn focus_panel_handle(focus: FocusHandle, window: &mut Window, on_next_frame: bool) {
    window.focus(&focus);
    if on_next_frame {
        window.on_next_frame(move |window, _| {
            window.focus(&focus);
        });
    }
}
