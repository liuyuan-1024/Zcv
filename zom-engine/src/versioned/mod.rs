//! Versioned Result 与 Versioned Range 公共载体。
//!
//! `VersionedResult<T>` 把任意 payload 与 BufferVersion 绑定；
//! `VersionedRangeSet<T>` 是带稳定 id 的 (TrackedRange, payload) 集合，供宿主分析产物、外部 range 标注复用。

pub(crate) mod query;
mod range_set;
mod result;

pub use range_set::{
    VersionedRangeEntry, VersionedRangeEntryId, VersionedRangeSet, VersionedRangeSpec,
    VersionedRangeUpdate,
};
pub use result::VersionedResult;
