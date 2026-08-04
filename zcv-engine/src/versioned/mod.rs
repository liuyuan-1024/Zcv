//! Versioned Result 公共载体。
//!
//! `VersionedResult<T>` 把任意 payload 与 BufferVersion 绑定，供搜索结果、变更区间等核心模块做版本守卫

mod result;

pub use result::VersionedResult;
