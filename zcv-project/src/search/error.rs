//! 搜索能力域错误：查询、匹配、替换与版本守卫的错误语义。

use thiserror::Error;
use zcv_text::{BufferVersion, CoordinateError, TextError};

/// `VersionedResult` 版本绑定与 remap 相关错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum VersionedResultError {
    /// 调用方传入的 DeltaEvent::old_version() 与 VersionedResult 当前绑定版本不一致。
    #[error("VersionedResult 版本不匹配：预期版本 {expected:?}，实际版本 {actual:?}")]
    VersionMismatch {
        expected: BufferVersion,
        actual: BufferVersion,
    },
}

/// 搜索查询、匹配与替换相关错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SearchError {
    /// 空 query 没有稳定的匹配语义。
    #[error("搜索 query 不能为空")]
    EmptyQuery,

    /// 正则表达式无法编译。
    #[error("非法正则表达式：pattern {pattern:?}，message {message}")]
    InvalidRegex { pattern: String, message: String },

    /// 搜索范围等坐标校验失败。
    #[error(transparent)]
    Coordinate(#[from] CoordinateError),

    /// 底层文本读取失败。
    #[error(transparent)]
    Text(#[from] TextError),

    /// 版本守卫失败。
    #[error(transparent)]
    Versioned(#[from] VersionedResultError),
}

/// 搜索能力域 Result 类型。
pub type SearchTextResult<T> = Result<T, SearchError>;
