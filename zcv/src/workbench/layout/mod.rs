//! 工作台布局系统 —— Dock + PaneGroup 分治。
//!
//! 布局控制分两类区域：
//!
//! - **Dock**（左/右/底）：可折叠，同一时间一个 panel 可见，用 PanelStack 切换。
//! - **中心编辑区**（PaneGroup）：递归分栏树，叶子是 Pane（一组 tab + 激活项）。
//!
//! `LayoutController` 是唯一的布局状态所有者与操作入口。

mod controller;
mod render;
mod types;

pub(crate) use controller::LayoutController;
pub(crate) use controller::LayoutRef;
pub(crate) use render::render_body;
pub(crate) use types::{LayoutSnapshot, PanelId, ViewId};
