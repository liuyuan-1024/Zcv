use std::time::Duration;

/// AI 请求或流式事件中可能出现的错误。
///
/// 每个变体对应一种上层 UI 决策路径，不要把语义压回到 `Other`。
#[derive(Debug)]
pub enum AiError {
    Network(String),
    Auth,
    RateLimited { retry_after: Option<Duration> },
    Timeout,
    ContextTooLong { limit_tokens: u32 },
    Cancelled,
    // provider 输出的 tool_use 不符合协议（缺字段、JSON 不合法等）。
    ToolProtocolViolation(String),
    ProviderUnavailable,
    Other(String),
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(msg) => write!(f, "网络错误：{msg}"),
            Self::Auth => f.write_str("AI 鉴权失败"),
            Self::RateLimited { retry_after } => match retry_after {
                Some(d) => write!(f, "AI 请求被限流，建议 {d:?} 后重试"),
                None => f.write_str("AI 请求被限流"),
            },
            Self::Timeout => f.write_str("AI 请求超时"),
            Self::ContextTooLong { limit_tokens } => {
                write!(f, "上下文超出模型限制（{limit_tokens} tokens）")
            }
            Self::Cancelled => f.write_str("AI 请求已取消"),
            Self::ToolProtocolViolation(msg) => write!(f, "AI 工具协议违例：{msg}"),
            Self::ProviderUnavailable => f.write_str("AI 服务提供方不可用"),
            Self::Other(msg) => write!(f, "AI 错误：{msg}"),
        }
    }
}

impl std::error::Error for AiError {}
