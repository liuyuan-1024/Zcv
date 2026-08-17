//! 工作区框架：Item 协议、Pane/Dock/StatusBar/Toolbar 与 Workspace 装配。
//! 此文件是 `zcv-workspace` crate 的公共入口。
//!
//! 对齐 Zed 的 workspace crate：标签页 Item 的通用能力、文件打开/预览的注册机制、以及编辑区布局（Pane/Dock）与工具条（Toolbar/StatusBar）框架。
//! 不依赖 Editor、具体预览格式（它们经 ItemProvider/PreviewProvider 注册接入）。

mod activity_indicator;
mod branch_picker;
mod dock;
mod item;
mod item_provider;
mod pane;
mod panel;
mod panel_buttons;
mod preview;
mod project_picker;
mod recent_projects;
mod search_bar;
mod searchable;
mod status_bar;
mod tab_bar;
mod toast;
mod toolbar;
mod top_bar;
mod window_controls;
mod workspace_state;

pub use activity_indicator::ActivityIndicator;
pub use branch_picker::{GitBranchAction, OnBranchSelected};
pub use dock::{
    Dock, DockPosition, ToggleDebug, ToggleDiagnostics, ToggleKeyboardShortcuts,
    ToggleLanguageServer, ToggleOutline, ToggleProjectSearch, ToggleProjectTree, ToggleTerminal,
    ToggleVersionControl,
};
pub use item::{Item, ItemEvent, ItemEventHandler, ItemHandle};
pub use item_provider::{ItemProvider, register_item_provider};
pub use pane::{Pane, PaneEvent};
pub use panel::{Panel, PanelHandle};
pub use panel_buttons::PanelButtons;
pub use preview::{PreviewDocument, PreviewItem, PreviewItemHandle, PreviewProvider, register};
pub use project_picker::OnProjectSelected;
pub use recent_projects::add_to_recent;
pub use searchable::{Direction, SearchEvent, SearchQuery, SearchableItem, SearchableItemHandle};
pub use status_bar::StatusItemView;
pub use toolbar::{FileToolbarControls, ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView};
pub use top_bar::TopBar;
pub use workspace_state::Workspace;
