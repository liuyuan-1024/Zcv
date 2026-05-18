//! L2 视觉原语 —— 无领域语义、可被 panels 与 workbench 复用。
//!
//! 依赖方向（手册 7.3）：只向下用 token；不向上 `use` panels / workbench。

mod bar_frame;
mod dock_frame;
mod glyph;
mod panel_placeholder;

pub(crate) use bar_frame::{BarEdge, BarRegionAlign, align_bar_region, bar_divider, bar_frame};
pub(crate) use dock_frame::{DockEdge, dock_frame};
pub(crate) use glyph::Glyph;
pub(crate) use panel_placeholder::panel_placeholder;
