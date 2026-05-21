//! Workbench overlay 框架（手册 21、布局模型 7）。
//!
//! Manager 作为 GPUI `Entity<OverlayManager>` 挂在 `ShellView` 上，命令系统
//! 只通过 `HostEffect` 请求打开某类 overlay；anchor 由 shell 按 kind 自行决定。

mod anchor;
pub(crate) mod bubble_layer;
mod manager;
mod shell;

use gpui::ElementId;

pub(crate) use anchor::{AnchorRegistry, track_anchor};
pub(crate) use manager::{ActiveOverlay, OverlayManager};
pub(crate) use shell::OverlayShell;

/// 当前活跃 overlay 的身份。第一版同时最多 1 个（手册 21.3）。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum OverlayKind {
    /// 顶栏"切换项目"入口的悬浮面板：最近项目列表 + 打开本地文件夹。
    ProjectPicker,
    /// 底栏"语言服务器"的悬浮面板。
    LanguageServers,
}

/// Overlay 的定位锚点（布局模型 7.x 列了 5 种）。第一版只用 `Element`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OverlayAnchor {
    /// 锚到某个已渲染 element 的矩形上（如顶栏 "zom" 标签）。
    Element(ElementId),
}
