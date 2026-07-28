//! Editor FoldSet：维护一组可跟随文本变化的折叠区间。
//!
//! 本模块只承担「哪些 byte range 是折叠状态」事实，以及 fold 集合自身的嵌套 / 重叠 /
//! 版本不变量；占位符样式、投影坐标和 viewport 切片由 `projection` 模块承载。

pub(crate) mod geometry;
mod hidden;
mod id;
mod range;
mod set;
mod update;

pub use hidden::HiddenRange;
pub use id::FoldRangeId;
pub use range::FoldRange;
pub use set::{FoldSet, FoldToggleOutcome};
pub(crate) use set::{HiddenSpan, HiddenSpanEnd};
pub use update::FoldRangeUpdate;
