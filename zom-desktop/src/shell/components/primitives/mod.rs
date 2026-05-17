//! L2 视觉原语 —— 无领域语义、可被 panels 与 regions 复用。
//!
//! 依赖方向（手册 7.3）：只向下用 token；不向上 `use` panels / regions。
//! 导出统一使用 `pub(in crate::shell::components)`，把可见性精确锁在
//! components 子树内（panels / regions 都在此子树）。

mod bar_frame;
mod dock_frame;
mod glyph;
mod panel_placeholder;

pub(in crate::shell::components) use bar_frame::{
    BarEdge, BarRegionAlign, align_bar_region, bar_divider, bar_frame,
};
pub(in crate::shell::components) use dock_frame::{DockEdge, dock_frame};
pub(in crate::shell::components) use glyph::Glyph;
pub(in crate::shell::components) use panel_placeholder::panel_placeholder;
