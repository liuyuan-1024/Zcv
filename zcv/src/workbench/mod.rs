//! WorkbenchFrame —— 窗口顶层装配。

mod bars;
mod layout;
pub(crate) mod project_tree;
mod workspace;

pub(crate) use bars::{
    bottom_bar, bottom_bar::BottomBar, top_bar, top_bar::TopBar, window_controls,
};
pub(crate) use layout::{
    CloseTab, LayoutController, LayoutRef, LayoutSnapshot, NextTab, Pane, PaneId, PanelId, PrevTab,
    ViewId, handle_close_tab, render_body as render_layout_body,
};
pub(crate) use project_tree::ProjectTree;
pub(crate) use workspace::Workspace;
