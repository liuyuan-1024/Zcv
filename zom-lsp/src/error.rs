//! LSP 协议层错误类型。
//!
//! 每个变体对应一条 UI 决策路径，让上层可以按错误分类做不同处置。

/// LSP 客户端可能遇到的错误。
#[derive(Debug)]
pub enum LspError {
    /// 语言服务器二进制未找到（`PATH` 中无此命令或路径不存在）。
    ServerNotFound { command: String },

    /// 服务器进程启动失败（权限不足、缺少运行时等）。
    ServerStartFailed { command: String, reason: String },

    /// 服务器进程非预期退出。
    ServerExited { command: String, exit: ExitStatus },

    /// JSON-RPC 编解码失败。
    ProtocolViolation { detail: String },

    /// 服务器返回 error response（`result` 为 null，`error` 非 null）。
    ServerError { code: i64, message: String },

    /// 请求在超时时间内未收到响应。
    Timeout { method: String },

    /// 通道关闭——底层 transport 断开或内部 channel 已关。
    ChannelClosed,
}

/// 进程退出状态快照。
#[derive(Debug, Clone)]
pub enum ExitStatus {
    Code(i32),
    Signal,
}

impl std::fmt::Display for LspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LspError::ServerNotFound { command } => {
                write!(f, "语言服务器未找到：{command}")
            }
            LspError::ServerStartFailed { command, reason } => {
                write!(f, "启动 {command} 失败：{reason}")
            }
            LspError::ServerExited { command, exit } => {
                write!(f, "{command} 已退出（{exit:?}）")
            }
            LspError::ProtocolViolation { detail } => {
                write!(f, "协议错误：{detail}")
            }
            LspError::ServerError { code, message } => {
                write!(f, "服务器错误 [{code}]：{message}")
            }
            LspError::Timeout { method } => {
                write!(f, "请求超时：{method}")
            }
            LspError::ChannelClosed => {
                write!(f, "通道已关闭")
            }
        }
    }
}

impl std::error::Error for LspError {}
