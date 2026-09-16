//! 工作区：Item 协议、Pane/Dock/StatusBar 与 Workspace 装配。
//! 此文件是 `zcv-workspace` crate 的公共入口。
//!
//! 负责 标签页 Item 的集成能力、文件打开/预览的注册机制，以及编辑区布局（Pane/Dock）与状态栏。
//! 具体 Editor 和预览格式通过 ItemProvider/PreviewProvider 注册接入。

use gpui::{App, Window};
use zcv_theme::typography::Typography;

mod activity_indicator;
mod branch_picker;
mod breadcrumbs;
mod dock;
mod item;
mod item_provider;
mod layout_state;
mod pane;
mod panel;
mod panel_buttons;
mod persistence;
mod preview;
mod project_picker;
mod provider_registry;
mod recent_projects;
mod searchable;
mod status_bar;
mod status_color;
mod tab_bar;
mod toast;
mod top_bar;
mod window_bounds;
mod workspace_state;

#[cfg(test)]
mod test;

pub use activity_indicator::ActivityIndicator;
pub use branch_picker::{GitBranchAction, OnBranchSelected};
pub use breadcrumbs::Breadcrumbs;
pub use dock::{Dock, DockPosition};
pub use item::{Item, ItemEvent, ItemHandle};
pub use item_provider::{
    ItemProvider, SerializedItemProvider, register_item_provider, register_serialized_item_provider,
};
pub use layout_state::SerializedPaneItem;
pub use pane::{Pane, PaneEvent};
pub use panel::{Panel, PanelEvent, PanelHandle};
pub use panel_buttons::PanelButtons;
pub use preview::{
    OpenPathCallback, PreviewButton, PreviewDocument, PreviewItem, PreviewItemHandle, PreviewMode,
    PreviewPresentation, PreviewProvider, PreviewToggleCallback, PreviewViewport,
    PreviewViewportOptions, register,
};
pub use project_picker::OnProjectSelected;
pub use recent_projects::{add_to_recent, most_recent_valid_project};
pub use searchable::{Direction, SearchEvent, SearchableItem, SearchableItemHandle};
pub use status_bar::StatusItemView;
pub use status_color::git_status_color;
pub use toast::{ToastAction, ToastKind};
pub use top_bar::{TopBar, TopBarCallbacks};
pub use window_bounds::{load_window_bounds, save_window_bounds};
pub use workspace_state::Workspace;

/// 读取窗口根工作区的排版快照。
///
/// 工作区内的视图通过窗口根实体取得会话级状态；
/// 独立挂载的组件（例如单元测试中的Editor）没有工作区时使用 zcv-theme 的基础快照。
pub fn typography_for_window(window: &Window, cx: &App) -> Typography {
    window
        .root::<Workspace>()
        .flatten()
        .map(|workspace| workspace.read(cx).typography())
        .unwrap_or_else(zcv_theme::typography::current)
}
