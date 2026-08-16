//! 工作区框架：Item 协议、Pane/Dock/StatusBar/Toolbar 与 Workspace 装配。
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
/// 预览协议与注册表；格式 crate（如 zcv-preview-svg）实现该协议。
pub mod preview;
mod project_picker;
mod recent_projects;
mod status_bar;
mod tab_bar;
mod toast;
mod toolbar;
mod top_bar;
mod window_controls;
mod workspace;

pub use activity_indicator::ActivityIndicator;
pub use branch_picker::{BranchPicker, GitBranchAction, OnSelectBranch};
pub use dock::{
    Dock, DockPosition, ToggleDebug, ToggleDiagnostics, ToggleKeyboardShortcuts,
    ToggleLanguageServer, ToggleOutline, ToggleProjectSearch, ToggleProjectTree, ToggleTerminal,
    ToggleVersionControl, render_body,
};
pub use item::{Item, ItemEvent, ItemEventHandler, ItemHandle, ToolbarItemLocation};
pub use item_provider::{
    ItemProvider, ItemProviderDescriptor, item_provider_for_path, register_item_provider,
};
pub use pane::{CloseTab, NextTab, Pane, PaneEvent, PrevTab, ToggleFileSearch, TogglePreview};
pub use panel::{Panel, PanelHandle};
pub use panel_buttons::PanelButtons;
pub use preview::{
    PreviewDescriptor, PreviewDocument, PreviewItem, PreviewItemHandle, PreviewProvider,
    PreviewProviderId, provider_for, register,
};
pub use project_picker::{OnProjectSelected, ProjectPicker};
pub use recent_projects::{
    ProjectEntry, add_to_recent, load_recent_projects, remove_from_recent, save_recent_projects,
};
pub use status_bar::{StatusBar, StatusItemView, StatusItemViewHandle};
pub use toast::{ToastAction, ToastKind, ToastLayer};
pub use toolbar::{
    FileToolbarControls, Toolbar, ToolbarItemEvent, ToolbarItemView, ToolbarItemViewHandle,
};
pub use top_bar::TopBar;
pub use window_controls::{
    handle_minimize, handle_quit, handle_toggle_maximize, render as render_window_controls,
};
pub use workspace::{GitFetch, GitPull, GitPush, OpenSettings, Save, Workspace};
