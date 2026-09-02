//! 工作区框架：Item 协议、Pane/Dock/StatusBar/Toolbar 与 Workspace 装配。
//! 此文件是 `zcv-workspace` crate 的公共入口。
//!
//! 标签页 Item 的通用能力、文件打开/预览的注册机制、以及编辑区布局（Pane/Dock）与工具条（Toolbar/StatusBar）框架。
//! 不依赖 Editor、具体预览格式（它们经 ItemProvider/PreviewProvider 注册接入）。

mod activity_indicator;
mod branch_picker;
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
mod tab_bar;
mod toast;
mod toolbar;
mod top_bar;
mod window_bounds;
mod window_controls;
mod workspace_state;

#[cfg(test)]
mod test;

pub use activity_indicator::ActivityIndicator;
pub use branch_picker::{GitBranchAction, OnBranchSelected};
pub use dock::{Dock, DockPosition};
pub use item::{Item, ItemEvent, ItemHandle};
pub use item_provider::{ItemProvider, register_item_provider};
pub use pane::{Pane, PaneEvent};
pub use panel::{Panel, PanelEvent, PanelHandle};
pub use panel_buttons::PanelButtons;
pub use preview::{
    PreviewButton, PreviewDocument, PreviewItem, PreviewItemHandle, PreviewProvider, register,
};
pub use project_picker::OnProjectSelected;
pub use recent_projects::{add_to_recent, most_recent_valid_project};
pub use searchable::{Direction, SearchEvent, SearchableItem, SearchableItemHandle};
pub use status_bar::StatusItemView;
pub use toast::{ToastAction, ToastKind};
pub use toolbar::{ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView};
pub use top_bar::TopBar;
pub use window_bounds::{load_window_bounds, save_window_bounds};
pub use workspace_state::Workspace;
