//! UI 模块身份枚举 —— crate 内最底层的「这是哪个 panel / surface」词汇表。
//!
//! 这些枚举是 *跨命令系统、布局、焦点* 共用的纯数据：
//! - [`focus`](crate::focus) 用它们做语义焦点的外标签；
//! - [`shell::features::panels`](crate::shell::features::panels) 在它们之上挂 panel 框架；
//! - [`shell::surfaces`](crate::shell::surfaces) 在它们之上挂 surface 管理。
//!
//! 放在 crate 顶层而不是 shell 内部，避免上层概念（focus）反向引用下层细节。

/// 桌面端 panel 列表。
pub(crate) use zom_command::PanelKind as PanelId;

/// 该 panel 在 bar 上代表的图标资源路径（embedded assets 里的相对路径）。
///
/// 图标属于纯 UI 关心，所以保持在 desktop 侧的 free fn，不污染 `PanelKind` 公共 API。
pub(crate) fn panel_icon_path(panel: PanelId) -> &'static str {
    match panel {
        PanelId::FileTree => "icons/panels/file_tree.svg",
        PanelId::VersionControl => "icons/panels/version_control.svg",
        PanelId::Outline => "icons/panels/outline.svg",
        PanelId::Terminal => "icons/panels/terminal.svg",
        PanelId::Debug => "icons/panels/debug.svg",
        PanelId::KeyboardShortcuts => "icons/panels/keyboard_shortcuts.svg",
    }
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
    /// 底栏"跳转到行"的浮面。
    GoToLine,
}
