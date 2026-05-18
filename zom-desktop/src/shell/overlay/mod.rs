//! Overlay 类型骨架（手册 21、布局模型 7）。
//!
//! Manager 作为 GPUI `Entity<OverlayManager>` 挂在 `ShellView` 上，命令系统
//! 只通过 `WindowAction::OpenOverlay(kind)` 请求打开某类 overlay；anchor 由
//! shell 按 kind 自行决定，不进入 `HostEffect`。
//!
//! 闭合枚举：新增 overlay 形态 = 在这里加变体 + 各处 match 编译报缺。

use gpui::ElementId;

mod anchor;
mod language_servers;
mod manager;
mod project_picker;
mod shell;
pub(crate) use anchor::{AnchorRegistry, track_anchor};
pub(crate) use manager::{ActiveOverlay, OverlayManager};
pub(crate) use shell::OverlayShell;

/// 当前活跃 overlay 的身份。第一版同时最多 1 个（手册 21.3）。
///
/// 变体按"落地优先级"排序；未启用的形态注释列出而非占变体，避免到处 match 写
/// `unreachable!` ——真用到时再加，编译器会把所有 match 点拉出来。
///
/// 计划中后续变体（手册 21.1 / 布局模型 7.x）：
/// `CommandPalette` / `QuickPicker` / `ContextMenu` / `InlinePopover` /
/// `InspectorPopover`。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum OverlayKind {
    /// 顶栏"切换项目"入口的悬浮面板：最近项目列表 + 打开本地文件夹。
    ProjectPicker,
    /// 底栏"语言服务器"的悬浮面板。
    LanguageServers,
}

/// Overlay 的定位锚点（布局模型 7.x 列了 5 种）。第一版只用 `Element`。
///
/// 计划中后续变体：
/// - `WindowCenter`：命令面板等居中场景。
/// - `Cursor`：右键菜单等跟随鼠标的场景。
/// - `EditorPosition { editor_id, line, column }`：编辑器内 inline popover。
/// - `Rect(ScreenRect)`：调用方自算好屏幕矩形直接传入。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OverlayAnchor {
    /// 锚到某个已渲染 element 的矩形上（如顶栏 "zom" 标签）。
    /// 具体角对齐 / 偏移在 `OverlayShell` 渲染期通过 GPUI `anchored()` 决定。
    Element(ElementId),
}
