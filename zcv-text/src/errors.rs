//! 文本内核错误边界：集中定义坐标、编辑、事务和存储四类底层错误及统一 TextError。
//!
//! 本文件只表达可匹配的失败语义，不携带 UI 文案、恢复策略或外部协议适配细节。

use thiserror::Error;

use crate::types::{BufferVersion, ByteOffset, CharOffset, Line, TextRange, Utf16Position};

/// 坐标转换、边界校验或越界相关的错误（坐标不合法）。
///
/// **坐标系唯一真理**：文本内核内部所有越界错误以 `ByteOffset` 描述；`CharOffset`
/// 相关变体仅用于边界投影路径（如 UTF-16 协议适配、外部坐标转换入口）。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoordinateError {
    /// ByteOffset 越界：超过当前 UTF-8 文本字节长度。
    #[error("字节偏移量越界：{0}")]
    OutOfBounds(ByteOffset),

    /// ByteOffset 落在 UTF-8 多字节序列中间，不构成合法字符边界。
    #[error("字节偏移量不在 UTF-8 字符边界：{0}")]
    InvalidByteBoundary(ByteOffset),

    /// 边界投影路径上的 `CharOffset` 越界。仅外部坐标转换入口使用。
    #[error("字符偏移量越界：{0}")]
    CharOutOfBounds(CharOffset),

    /// UTF-16 行列位置超出当前文本的行数或行内 code unit 范围。
    #[error("UTF-16 位置越界：{0:?}")]
    Utf16PositionOutOfBounds(Utf16Position),

    /// UTF-16 位置切进 surrogate pair 中间，不能表示为文本内核的 byte 坐标。
    #[error("UTF-16 位置落在代理对中间：{0:?}")]
    InvalidUtf16Boundary(Utf16Position),

    /// 逻辑行号不存在；是否允许等于 line_count 由具体 API 的半开边界语义决定。
    #[error("行索引越界：{0:?}")]
    LineOutOfBounds(Line),

    /// 调用方传入了反向 TextRange；TextRange public 构造器必须拒绝该状态。
    #[error("非法文本区间：start {start} 大于 end {end}")]
    InvalidRange { start: ByteOffset, end: ByteOffset },

    /// 调用方传入了反向 LineRange；行窗口查询必须保持 `[start, end)` 不变量。
    #[error("非法行区间：start {start:?} 大于 end {end:?}")]
    InvalidLineRange { start: Line, end: Line },

    /// ByteOffset 是合法字符边界，但不是合法 grapheme 边界，不能用于用户感知移动/切分。
    #[error("字节偏移处的字素边界无效：{0}")]
    InvalidGraphemeBoundary(ByteOffset),
}

/// 文本变异与编辑相关的错误（编辑请求不合法）。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    /// 同一事务内两个编辑范围相交；EditList 必须在提交前完成排序和重叠拒绝。
    #[error("检测到重叠编辑：之前 {previous:?}，当前 {current:?}")]
    OverlappingEdits {
        previous: TextRange,
        current: TextRange,
    },

    /// 编辑范围满足 TextRange 自身不变量，但超出当前 Buffer 文本长度。
    #[error("编辑区间越界：{range:?}")]
    RangeOutOfBounds { range: TextRange },

    /// 编辑端点不是当前阶段要求的文本边界，例如落在 UTF-8 多字节序列或
    /// grapheme 中间的组合输入范围。
    #[error("编辑区间落在非法文本边界：{offset}")]
    InvalidBoundary { offset: ByteOffset },

    /// 单次编辑携带的 replacement 太大，应由调用方分块或在更高层拒绝操作。
    #[error("编辑有效载荷超过最大允许大小：当前大小 {size}，限制 {limit}")]
    PayloadTooLarge { size: usize, limit: usize },
}

/// 事务提交与管理相关的错误（事务提交不合法）。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransactionError {
    /// Transaction 必须至少包含一个编辑；空编辑列表只允许停留在 EditList 层。
    #[error("事务为空")]
    EmptyTransaction,

    /// Transaction 绑定的 base_version 与 Buffer 当前版本不同，提交必须原子拒绝。
    #[error("版本不匹配：预期版本 {expected:?}，实际版本 {actual:?}")]
    VersionMismatch {
        expected: BufferVersion,
        actual: BufferVersion,
    },
}

/// Anchor / Mark 版本推进相关错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnchorError {
    /// Anchor / Mark 只能通过连续 DeltaEvent 推进，不能跳过或重复应用版本。
    #[error("Anchor 版本不匹配：预期版本 {expected:?}，实际版本 {actual:?}")]
    VersionMismatch {
        expected: BufferVersion,
        actual: BufferVersion,
    },
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
}

/// 文本内核统一错误类型。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TextError {
    /// 坐标或边界校验失败，通常可以通过修正调用方坐标恢复。
    #[error(transparent)]
    Coordinate(#[from] CoordinateError),

    /// 编辑请求本身不合法，事务提交前应保持 Buffer 完全不变。
    #[error(transparent)]
    Edit(#[from] EditError),

    /// 事务版本、内容或历史节点不满足提交/回放契约。
    #[error(transparent)]
    Transaction(#[from] TransactionError),

    /// Anchor 的版本推进失败。
    #[error(transparent)]
    Anchor(#[from] AnchorError),

    /// 底层文本存储或加载边界失败。
    #[error(transparent)]
    Storage(#[from] StorageError),

    /// BufferVersion 递增越过 u64 上限；调用方应创建新 Buffer 生命周期。
    #[error("BufferVersion 溢出")]
    VersionOverflow,

    /// TransactionId 递增越过 u64 上限；历史系统不能继续生成唯一事务事实。
    #[error("TransactionId 溢出")]
    TransactionIdOverflow,

    /// HistoryNodeId 计数器耗尽；历史系统不能再分配唯一节点身份。
    #[error("HistoryNodeId 溢出")]
    HistoryIdExhausted,

    /// 编辑日志已裁剪掉请求版本，无法重建该版本到当前的编辑；调用方必须回退。
    #[error("编辑日志已裁剪版本：请求版本 {requested:?}，最早可查版本 {earliest:?}")]
    VersionEvicted {
        requested: BufferVersion,
        earliest: BufferVersion,
    },

    /// 文本内核内部不变量被违反；这是 bug，不是可恢复的外部错误。
    /// 用 `location` 定位代码点，`detail` 携带最少诊断信息，便于宿主上报。
    #[error("文本内核不变量违反：{location}：{detail}")]
    InvariantViolation {
        location: &'static str,
        detail: String,
    },
}

/// 文本内核统一 Result 类型。
pub type TextResult<T> = Result<T, TextError>;

/// 文本内核内部不变量断言：违反即 panic（这是 bug，不是可恢复的外部错误）。
///
/// 用法与 `Option::expect` 相同，但 panic 消息统一携带源码位置与明细，便于按文件名与行号检索文本内核 bug 的触发点。
macro_rules! invariant {
    ($option:expr, $detail:expr) => {
        match $option {
            Some(value) => value,
            None => panic!(
                "文本内核不变量违反（{}:{}）：{}",
                file!(),
                line!(),
                $detail
            ),
        }
    };
}
pub(crate) use invariant;

#[cfg(test)]
mod tests {
    use super::*;

    fn b(value: usize) -> ByteOffset {
        ByteOffset::new(value)
    }

    #[test]
    fn domain_errors_should_lift_into_text_error_without_losing_variant() {
        let coordinate: TextError = CoordinateError::OutOfBounds(b(99)).into();
        let edit: TextError = EditError::PayloadTooLarge { size: 9, limit: 3 }.into();
        let transaction: TextError = TransactionError::EmptyTransaction.into();

        assert!(matches!(
            coordinate,
            TextError::Coordinate(CoordinateError::OutOfBounds(offset)) if offset == b(99)
        ));
        assert!(matches!(
            edit,
            TextError::Edit(EditError::PayloadTooLarge { size: 9, limit: 3 })
        ));
        assert!(matches!(
            transaction,
            TextError::Transaction(TransactionError::EmptyTransaction)
        ));
    }
}
