//! 文本加载编码策略：描述 UTF-8、BOM 和非法字节的处理选择。
//!
//! 本文件只定义策略词汇；实际 bytes 解码在 Buffer 加载流程中完成。

/// 当前支持的原始文本编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextEncoding {
    /// 当前唯一承诺的加载编码；其他编码应在进入 Buffer 前由宿主转成 UTF-8。
    #[default]
    Utf8,
}

/// UTF-8 BOM 进入 Buffer 文本时的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BomPolicy {
    /// 识别并移除 UTF-8 BOM，同时在 `LoadedTextInfo` 中记录它存在过。
    #[default]
    Strip,
    /// 把 BOM 作为 U+FEFF 保留在 Buffer 文本中。
    Preserve,
}

/// 非法 UTF-8 字节的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum InvalidUtf8Policy {
    /// 遇到非法 UTF-8 直接返回错误。
    #[default]
    Reject,
    /// 使用 Unicode replacement character 恢复为可编辑文本。
    Replace,
}
