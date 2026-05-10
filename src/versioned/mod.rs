//! M14 Versioned Result 与 External Range Primitives 公共载体。
//!
//! 当前实现 M14A 的 `VersionedResult<T>` 与 M14B 的 `VersionedRangeSet<T>`；
//! M14C UTF-16 边界 helper 留待后续扩展。

mod range_set;
mod result;

pub use range_set::{VersionedRangeEntry, VersionedRangeSet, VersionedRangeSpec};
pub use result::VersionedResult;
