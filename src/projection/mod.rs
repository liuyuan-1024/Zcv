//! M13B Projection Line Map：从 `Snapshot + FoldSet` 投影出可见行序列与逻辑行映射。
//!
//! Projection 只承担行级别的折叠展开数学：
//! - 投影行索引与逻辑行号之间的双向映射；
//! - 折叠占位符行（`FoldPlaceholder`）的位置与覆盖的隐藏行区间。
//!
//! Point/Range 级别的投影映射属于 M13C，viewport 切片属于 M13D，本文件不承诺这些能力。

mod index;
mod line;
mod point;
mod projection;
mod range;

pub use index::ProjectedLineIndex;
pub use line::{FoldPlaceholder, LogicalProjection, ProjectedLine, ProjectedLineKind, TextLine};
pub use point::{LogicalPoint, LogicalPointProjection, ProjectedPoint, ProjectedPointMapping};
pub use projection::Projection;
pub use range::{LogicalRange, ProjectedRange};
