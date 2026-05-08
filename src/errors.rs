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
    /// CharOffset 超过当前文本长度，或不能作为当前查询入口的合法 char 坐标。
    #[error("字符偏移量越界: {0:?}")]
    OutOfBounds(CharOffset),

    /// ByteOffset 超过 UTF-8 存储字节长度；通常只应出现在编码/外部协议适配边界。
    #[error("字节偏移量越界: {0:?}")]
    ByteOutOfBounds(ByteOffset),

    /// ByteOffset 落在 UTF-8 多字节序列中间，不能安全转换为 CharOffset。
    #[error("字节偏移量不在 UTF-8 字符边界: {0:?}")]
    InvalidByteBoundary(ByteOffset),

    /// 调用方传入了反向字节区间；byte range 只用于外部协议 / 文件适配读取边界。
    #[error("非法字节区间: start {start:?} 大于 end {end:?}")]
    InvalidByteRange { start: ByteOffset, end: ByteOffset },

    /// UTF-16 行列位置超出当前文本的行数或行内 code unit 范围。
    #[error("UTF-16 位置越界: {0:?}")]
    Utf16PositionOutOfBounds(Utf16Position),

    /// UTF-16 位置切进 surrogate pair 中间，不能表示为引擎的 char 坐标。
    #[error("UTF-16 位置落在代理对中间: {0:?}")]
    InvalidUtf16Boundary(Utf16Position),

    /// 逻辑行号不存在；是否允许等于 line_count 由具体 API 的半开边界语义决定。
    #[error("行索引越界: {0:?}")]
    LineOutOfBounds(Line),

    /// 调用方传入了反向 TextRange；TextRange public 构造器必须拒绝该状态。
    #[error("非法文本区间: start {start:?} 大于 end {end:?}")]
    InvalidRange { start: CharOffset, end: CharOffset },

    /// 调用方传入了反向 LineRange；行窗口查询必须保持 `[start, end)` 不变量。
    #[error("非法行区间: start {start:?} 大于 end {end:?}")]
    InvalidLineRange { start: Line, end: Line },

    /// CharOffset 是合法字符边界，但不是合法 grapheme 边界，不能用于用户感知移动/切分。
    #[error("字符偏移处的字素边界无效: {0:?}")]
    InvalidGraphemeBoundary(CharOffset),
}

/// 文本变异与编辑相关的错误（编辑请求不合法）。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    /// 同一事务内两个编辑范围相交；M2 要求 EditList 在提交前完成排序和重叠拒绝。
    #[error("检测到重叠编辑: 之前 {previous:?}, 当前 {current:?}")]
    OverlappingEdits {
        previous: TextRange,
        current: TextRange,
    },

    /// 编辑范围满足 TextRange 自身不变量，但超出当前 Buffer 文本长度。
    #[error("编辑区间越界: {range:?}")]
    RangeOutOfBounds { range: TextRange },

    /// 编辑端点不是当前阶段要求的文本边界，例如落在 grapheme 中间的组合输入范围。
    #[error("编辑区间落在非法文本边界: {offset:?}")]
    InvalidBoundary { offset: CharOffset },

    /// 单次编辑携带的 replacement 太大，应由调用方分块或在更高层拒绝操作。
    #[error("编辑有效载荷超过最大允许大小: 当前大小 {size}, 限制 {limit}")]
    PayloadTooLarge { size: usize, limit: usize },
}

/// 事务提交与管理相关的错误（事务提交不合法）。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransactionError {
    /// 事务 ID 指向的历史节点不存在或已不再满足历史系统内部不变量。
    #[error("事务 {0:?} 无效或已损坏")]
    InvalidTransaction(TransactionId),

    /// Transaction 必须至少包含一个编辑；空编辑列表只允许停留在 EditList 层。
    #[error("事务为空")]
    EmptyTransaction,

    /// Transaction 绑定的 base_version 与 Buffer 当前版本不同，提交必须原子拒绝。
    #[error("版本不匹配: 预期版本 {expected:?}，实际版本 {actual:?}")]
    VersionMismatch {
        expected: BufferVersion,
        actual: BufferVersion,
    },
}

/// Anchor / Mark 版本推进相关错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnchorError {
    /// Anchor / Mark 只能通过连续 DeltaEvent 推进，不能跳过或重复应用版本。
    #[error("Anchor 版本不匹配: 预期版本 {expected:?}，实际版本 {actual:?}")]
    VersionMismatch {
        expected: BufferVersion,
        actual: BufferVersion,
    },

    /// TrackedRange 的两个端点必须来自同一旧版本，否则无法定义一次一致的范围映射。
    #[error("TrackedRange 两端的 Anchor 版本不一致: start {start:?}, end {end:?}")]
    RangeVersionMismatch {
        start: BufferVersion,
        end: BufferVersion,
    },
}

/// MetadataLayer 承载与版本推进相关错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MetadataError {
    /// 单个 MetadataLayer 的 range id 计数器耗尽；这表示 layer 生命周期需要重建。
    #[error("MetadataLayer range id 溢出")]
    IdOverflow,

    /// MetadataLayer 只能应用同一 base_version 的 DeltaEvent，过期结果应由宿主替换或丢弃。
    #[error("MetadataLayer 版本不匹配: 预期版本 {expected:?}，实际版本 {actual:?}")]
    VersionMismatch {
        expected: BufferVersion,
        actual: BufferVersion,
    },
}

/// 当前 Buffer 内搜索相关错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SearchError {
    /// 空 query 没有稳定的匹配语义，调用方应在 UI / 宿主层决定如何展示空搜索。
    #[error("搜索 query 不能为空")]
    EmptyQuery,

    /// 搜索结果必须基于 Buffer 当前版本，过期结果不能继续用于替换。
    #[error("搜索结果版本不匹配: 预期版本 {expected:?}，实际版本 {actual:?}")]
    VersionMismatch {
        expected: BufferVersion,
        actual: BufferVersion,
    },

    /// 调用方请求替换不存在的搜索匹配序号。
    #[error("搜索匹配不存在: ordinal {ordinal}")]
    MatchNotFound { ordinal: usize },
}

/// 底层存储相关的错误（存储后端做不了）。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StorageError {
    /// 存储后端无法分配本次操作需要的内存，调用方不应假设 Buffer 状态已经改变。
    #[error("无法为大文件分配内存")]
    OutOfMemory,

    /// 当前存储实例拒绝变异操作；这是存储能力边界，不是文件系统权限错误。
    #[error("只读模式下不支持此操作")]
    ReadOnly,

    /// 外部 bytes 不能按当前 UTF-8 策略进入 Buffer，字段语义与 `std::str::Utf8Error` 对齐。
    #[error("输入不是合法 UTF-8: valid_up_to {valid_up_to}, error_len {error_len:?}")]
    InvalidUtf8 {
        valid_up_to: usize,
        error_len: Option<usize>,
    },

    /// 调用了该存储后端当前明确不承诺的能力，用于保护 trait 演进期的边界。
    #[error("存储后端不支持该操作: {0}")]
    UnsupportedOperation(&'static str),
}

/// 编辑引擎统一错误类型。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EngineError {
    /// 坐标或边界校验失败，通常可以通过修正调用方坐标恢复。
    #[error(transparent)]
    Coordinate(#[from] CoordinateError),

    /// 编辑请求本身不合法，事务提交前应保持 Buffer 完全不变。
    #[error(transparent)]
    Edit(#[from] EditError),

    /// 事务版本、内容或历史节点不满足提交/回放契约。
    #[error(transparent)]
    Transaction(#[from] TransactionError),

    /// Anchor 或 TrackedRange 的版本推进失败。
    #[error(transparent)]
    Anchor(#[from] AnchorError),

    /// MetadataLayer 与 Buffer 版本或 range 身份管理不一致。
    #[error(transparent)]
    Metadata(#[from] MetadataError),

    /// 当前 Buffer 内搜索请求不合法。
    #[error(transparent)]
    Search(#[from] SearchError),

    /// 底层文本存储或加载边界失败。
    #[error(transparent)]
    Storage(#[from] StorageError),

    /// BufferVersion 递增越过 u64 上限；调用方应创建新 Buffer 生命周期。
    #[error("BufferVersion 溢出")]
    VersionOverflow,

    /// TransactionId 递增越过 u64 上限；历史系统不能继续生成唯一事务事实。
    #[error("TransactionId 溢出")]
    TransactionIdOverflow,
}

/// 编辑引擎统一 Result 类型。
pub type EngineResult<T> = Result<T, EngineError>;
