//! UI 模块身份枚举 —— crate 内最底层的「这是哪个 panel / surface」词汇表。
//!
//! 这些枚举是 *跨命令系统、布局、焦点* 共用的纯数据：
//! - [`focus`](crate::focus) 用它们做语义焦点的外标签；
//! - [`shell::features::panels`](crate::shell::features::panels) 在它们之上挂 panel 框架；
//! - [`shell::surfaces`](crate::shell::surfaces) 在它们之上挂 surface 管理。
//!
//! 放在 crate 顶层而不是 shell 内部，避免上层概念（focus）反向引用下层细节。
//! 本模块不依赖 GPUI，也不依赖 zom-command 之外的外部 crate。

/// 桌面端当前固定的 panel 列表（手册 20.10）。
///
/// 不抽 `PanelProvider` trait（手册 20.2）；新增 panel 直接在此 enum 加变体。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum PanelId {
    FileTree,
    VersionControl,
    Outline,
    Search,
    Terminal,
    Debug,
    KeyboardShortcuts,
}

impl PanelId {
    /// 该 panel 在 bar 上代表的图标资源路径（embedded assets 里的相对路径）。
    pub(crate) fn icon_path(self) -> &'static str {
        match self {
            PanelId::FileTree => "icons/panels/file_tree.svg",
            PanelId::VersionControl => "icons/panels/version_control.svg",
            PanelId::Outline => "icons/panels/outline.svg",
            PanelId::Search => "icons/panels/search.svg",
            PanelId::Terminal => "icons/panels/terminal.svg",
            PanelId::Debug => "icons/panels/debug.svg",
            PanelId::KeyboardShortcuts => "icons/panels/keyboard_shortcuts.svg",
        }
    }

    /// 切换本 panel 显隐的完整命令 id。常量本体在各自
    /// `zom_command::commands::<feature>` 模块，这里只做枚举 → 常量 的映射，
    /// 供 bar glyph 等 UI 标注。
    pub(crate) fn toggle_command_id(self) -> &'static str {
        use zom_command::commands::{
            debug, file_tree, keyboard_shortcuts, outline, search, terminal, version_control,
        };
        match self {
            PanelId::FileTree => file_tree::TOGGLE_PANEL,
            PanelId::VersionControl => version_control::TOGGLE_PANEL,
            PanelId::Outline => outline::TOGGLE_PANEL,
            PanelId::Search => search::TOGGLE_PANEL,
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
            PanelId::Search => "search",
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
        PanelId::Search,
        PanelId::Terminal,
        PanelId::Debug,
        PanelId::KeyboardShortcuts,
    ];
}

/// 当前活跃 surface 的身份。它只用于高亮入口、去重与测试，不决定内容。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum SurfaceId {
    /// 顶栏"切换项目"入口的浮面：最近项目列表 + 打开本地文件夹。
    ProjectPicker,
    /// 顶栏"设置"入口的浮面。
    Settings,
    /// 底栏"语言服务器"的浮面。
    LanguageServers,
}
