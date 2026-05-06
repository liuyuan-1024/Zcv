//! 引擎错误边界：集中定义坐标、编辑、事务和存储四类底层错误及统一 EngineError。
//!
//! 本文件只表达可匹配的失败语义，不携带 UI 文案、恢复策略或外部协议适配细节。

use thiserror::Error;

use crate::types::{
    BufferVersion, ByteOffset, CharOffset, Line, TextRange, TransactionId, Utf16Position,
};

/// 坐标转换、边界校验或越界相关的错误（坐标不合法）。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoordinateError {
    #[error("字符偏移量越界: {0:?}")]
    OutOfBounds(CharOffset),

    #[error("字节偏移量越界: {0:?}")]
    ByteOutOfBounds(ByteOffset),

    #[error("字节偏移量不在 UTF-8 字符边界: {0:?}")]
    InvalidByteBoundary(ByteOffset),

    #[error("UTF-16 位置越界: {0:?}")]
    Utf16PositionOutOfBounds(Utf16Position),

    #[error("UTF-16 位置落在代理对中间: {0:?}")]
    InvalidUtf16Boundary(Utf16Position),

    #[error("行索引越界: {0:?}")]
    LineOutOfBounds(Line),

    #[error("非法文本区间: start {start:?} 大于 end {end:?}")]
    InvalidRange { start: CharOffset, end: CharOffset },

    #[error("字符偏移处的字素边界无效: {0:?}")]
    InvalidGraphemeBoundary(CharOffset),
}

/// 文本变异与编辑相关的错误（编辑请求不合法）。
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
    InvalidBoundary { offset: CharOffset },

    #[error("编辑有效载荷超过最大允许大小: 当前大小 {size}, 限制 {limit}")]
    PayloadTooLarge { size: usize, limit: usize },
}

/// 事务提交与管理相关的错误（事务提交不合法）。
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

/// 底层存储相关的错误（存储后端做不了）。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StorageError {
    #[error("无法为大文件分配内存")]
    OutOfMemory,

    #[error("只读模式下不支持此操作")]
    ReadOnly,

    #[error("输入不是合法 UTF-8: valid_up_to {valid_up_to}, error_len {error_len:?}")]
    InvalidUtf8 {
        valid_up_to: usize,
        error_len: Option<usize>,
    },

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

    #[error("BufferVersion 溢出")]
    VersionOverflow,

    #[error("TransactionId 溢出")]
    TransactionIdOverflow,
}

/// 编辑引擎统一 Result 类型。
pub type EngineResult<T> = Result<T, EngineError>;
