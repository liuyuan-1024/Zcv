//! Versioned Result 与 External Range Primitives 公共载体。
//!
//! `VersionedResult<T>` 把任意 payload 与 BufferVersion 绑定；`VersionedRangeSet<T>`
//! 是不带 layer kind / 稳定 ID 的轻量泛型 (TrackedRange, payload) 集合。两者都提供
//! snapshot-bound payload 转换与 UTF-16 边界 helper。

mod range_set;
mod result;

pub use range_set::{VersionedRangeEntry, VersionedRangeSet, VersionedRangeSpec};
pub use result::VersionedResult;
