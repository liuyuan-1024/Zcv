//! M14 Versioned Result 与 External Range Primitives 公共载体。
//!
//! 当前只实现 M14A 的 `VersionedResult<T>`；M14B/C 的 range set 与 UTF-16 边界 helper
//! 留待后续阶段扩展。

mod result;

pub use result::VersionedResult;
