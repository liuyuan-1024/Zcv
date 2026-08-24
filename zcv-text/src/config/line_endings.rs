//! 换行输出与外部 position 编码策略。
//!
//! 这些策略只影响保存/适配边界；核心编辑坐标仍保持 ByteOffset/TextRange。

/// Buffer 输出或规范化文本时采用的换行策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEndingConfig {
    /// 强制把换行写成 LF (`\n`)，适合希望统一仓库文本格式的调用方。
    Lf,
    /// 强制把换行写成 CRLF (`\r\n`)，适合需要 Windows 文本约定的调用方。
    Crlf,
    /// 保留加载时检测到的主要换行风格；新 Buffer 没有来源时由调用方再决定。
    Preserve,
    /// 使用当前平台的原生换行风格，适合宿主想跟随运行环境而不是文件来源时。
    Native,
}
