//! Workbench bar 系统的共享小件。
//!
//! 顶栏、底栏共用的外壳、对齐区和分隔线收在这里。

mod bottom_bar;
mod frame;
mod top_bar;
mod window_controls;

pub(crate) use bottom_bar::render as render_bottom_bar;
pub(crate) use top_bar::render as render_top_bar;
pub(crate) use window_controls::WindowControlsHandlers;
