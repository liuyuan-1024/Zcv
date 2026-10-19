//! 换行风格事实：记录文本中实际检测到的行结束符形态。
//!
//! 它是文本中实际检测到的事实；`Buffer` 的权威文本在入口统一为 LF。

/// 文本中检测到的换行风格。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LineEndingStyle {
    /// 文本中没有出现换行符。
    #[default]
    None,
    /// 只检测到 LF (`\n`)。
    Lf,
    /// 只检测到 CRLF (`\r\n`)。
    Crlf,
    /// 同时出现多种换行风格，或出现孤立 CR。
    Mixed,
}
