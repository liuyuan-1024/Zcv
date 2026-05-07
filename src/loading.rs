//! 文件文本加载边界：定义 UTF-8 bytes 进入 Buffer 时保留下来的编码与文本形态元信息。
//!
//! 本文件只描述加载结果和策略词汇，不执行 I/O、不保存文件，也不参与 reload 冲突处理。

use crate::LineEndingStyle;

/// 当前 M7C 支持的原始文本编码。
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

/// 一次外部文本加载留下的元信息。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LoadedTextInfo {
    pub encoding: TextEncoding,
    pub bom_policy: BomPolicy,
    pub invalid_utf8_policy: InvalidUtf8Policy,
    pub had_bom: bool,
    pub had_invalid_utf8: bool,
    pub line_ending_style: LineEndingStyle,
    pub has_final_newline: bool,
}

impl LoadedTextInfo {
    pub const fn new(
        encoding: TextEncoding,
        bom_policy: BomPolicy,
        invalid_utf8_policy: InvalidUtf8Policy,
        had_bom: bool,
        had_invalid_utf8: bool,
        line_ending_style: LineEndingStyle,
        has_final_newline: bool,
    ) -> Self {
        Self {
            encoding,
            bom_policy,
            invalid_utf8_policy,
            had_bom,
            had_invalid_utf8,
            line_ending_style,
            has_final_newline,
        }
    }
}
