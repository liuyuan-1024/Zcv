//! Workbench dock 系统。
//!
//! 左、右、底部 dock 的布局、resize 交互与 panel 分派都集中在这里。

pub(crate) mod bottom;
mod frame;
pub(crate) mod left;
mod panel_host;
pub(crate) mod resize;
pub(crate) mod right;

use frame::{DockEdge, dock_frame};

pub(crate) use panel_host::{PanelContext, PanelHost};
