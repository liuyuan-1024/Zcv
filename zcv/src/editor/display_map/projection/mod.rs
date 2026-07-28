//! Editor Projection：从 `Snapshot + FoldSet` 投影出可见行序列、逻辑行映射、point/range
//! 双向映射与折叠后视口切片。
//!
//! - 基于逻辑输入行数 / 投影输出行数双维摘要的双向映射；
//! - 折叠占位符行（`FoldPlaceholder`）的位置与覆盖的隐藏行区间；
//! - 最宽可见文本投影行的增量显示列摘要；
//! - `LogicalPoint` ↔ `ProjectedPoint`、`LogicalRange` ↔ `ProjectedRange` 双向映射；
//! - `ProjectedViewport` 折叠后视口切片。

mod core;
mod index;
mod line;
mod point;
mod range;
mod viewport;

pub use core::{ApplyOutcome, Projection};
pub use index::ProjectedLineIndex;
pub use line::{FoldPlaceholder, LogicalProjection, ProjectedLine, ProjectedLineKind, TextLine};
pub use point::{LogicalPoint, LogicalPointProjection, ProjectedPoint, ProjectedPointMapping};
pub use range::{LogicalRange, ProjectedRange};
pub use viewport::{
    ProjectedLineRange, ProjectedViewport, ProjectedViewportRow, ProjectedViewportRowKind,
    ProjectedViewportSlice,
};
