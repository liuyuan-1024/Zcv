//! 换行输出与外部 position 编码策略。
//!
//! 这些策略只影响保存/适配边界；核心编辑坐标仍保持 CharOffset/TextRange。

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

/// 与外部协议交换 position 时使用的行内坐标编码。
///
/// 引擎内部编辑坐标仍然是 CharOffset；这个策略只影响对外转换边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionEncodingConfig {
    /// 使用 UTF-8 byte offset 作为行内坐标，适配部分终端或低层文本协议。
    Utf8,
    /// 使用 UTF-16 code unit 作为行内坐标，适配 LSP 等编辑器协议。
    Utf16,
    /// 使用 Unicode scalar value 数量作为行内坐标，最接近引擎内部 logical column。
    Utf32,
}
