//! Workbench bar 系统的共享小件。
//!
//! 顶栏、底栏与编辑区标签栏共用的外壳、对齐区和 glyph 都收在这里。

mod frame;
mod glyph;

pub(crate) use frame::{BarEdge, BarRegionAlign, align_bar_region, bar_divider, bar_frame};
pub(crate) use glyph::Glyph;
