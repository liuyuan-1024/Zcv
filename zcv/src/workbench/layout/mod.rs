//! 工作台布局系统 —— Dock + PaneGroup 分治。
//!
//! 布局控制分两类区域：
//!
//! - **Dock**（左/右/底）：可折叠，同一时间一个 panel 可见，用 PanelStack 切换。
//! - **中心编辑区**（PaneGroup）：递归分栏树，叶子是 Pane（Entity，自带 FocusHandle）。

mod controller;
mod pane;
mod render;
mod types;

pub(crate) use controller::{LayoutController, LayoutRef, LayoutSnapshot, handle_close_tab};
pub(crate) use pane::{CloseTab, NextTab, Pane, PrevTab};
pub(crate) use render::render_body;
pub(crate) use types::{PaneId, PanelId, ViewId};
