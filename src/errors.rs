use crate::types::{BufferVersion, ByteOffset, Line, TextRange, TransactionId};
use thiserror::Error;

/// 坐标转换、边界校验或越界相关的错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoordinateError {
    #[error("字节偏移量越界: {0:?}")]
    OutOfBounds(ByteOffset),

    #[error("行索引越界: {0:?}")]
    LineOutOfBounds(Line),

    #[error("非法文本区间: start {start:?} 大于 end {end:?}")]
    InvalidRange { start: ByteOffset, end: ByteOffset },

    #[error("字节偏移量处的 UTF-8 边界无效: {0:?}")]
    InvalidUtf8Boundary(ByteOffset),

    #[error("字节偏移处的字素边界无效: {0:?}")]
    InvalidGraphemeBoundary(ByteOffset),
}

/// 文本变异与编辑相关的错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("检测到重叠编辑: 之前 {previous:?}, 当前 {current:?}")]
    OverlappingEdits {
        previous: TextRange,
        current: TextRange,
    },

    #[error("编辑区间越界: {range:?}")]
    RangeOutOfBounds { range: TextRange },

    #[error("编辑区间落在非法文本边界: {offset:?}")]
    InvalidBoundary { offset: ByteOffset },

    #[error("编辑有效载荷超过最大允许大小: 当前大小 {size}, 限制 {limit}")]
    PayloadTooLarge { size: usize, limit: usize },
}

/// 事务提交与管理相关的错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransactionError {
    #[error("事务 {0:?} 无效或已损坏")]
    InvalidTransaction(TransactionId),

    #[error("事务为空")]
    EmptyTransaction,

    #[error("版本不匹配: 预期版本 {expected:?}，实际版本 {actual:?}")]
    VersionMismatch {
        expected: BufferVersion,
        actual: BufferVersion,
    },
}

/// 底层存储相关的错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StorageError {
    #[error("无法为大文件分配内存")]
    OutOfMemory,

    #[error("只读模式下不支持此操作")]
    ReadOnly,

    #[error("存储后端不支持该操作: {0}")]
    UnsupportedOperation(&'static str),
}

/// 编辑引擎统一错误类型。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EngineError {
    #[error(transparent)]
    Coordinate(#[from] CoordinateError),

    #[error(transparent)]
    Edit(#[from] EditError),

    #[error(transparent)]
    Transaction(#[from] TransactionError),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// 编辑引擎统一 Result 类型。
pub type EngineResult<T> = Result<T, EngineError>;
