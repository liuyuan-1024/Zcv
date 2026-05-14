//! 换行风格事实：记录文本中实际检测到的行结束符形态。
//!
//! 它不同于 LineEndingConfig；前者是文本事实，后者是保存/规范化策略。

/// 文件中检测到的换行风格。
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
