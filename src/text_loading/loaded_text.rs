//! LoadedTextInfo：记录外部 bytes 进入 Buffer 后保留下来的文本形态事实。
//!
//! 这些字段用于保存、reload 和宿主提示，不等同于 Buffer 的当前 dirty 状态。

use crate::{BomPolicy, InvalidUtf8Policy, LineEndingStyle, TextEncoding};

/// 一次外部文本加载留下的元信息。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LoadedTextInfo {
    /// 原始 bytes 被接受为哪种编码；当前只承诺 UTF-8。
    pub encoding: TextEncoding,
    /// BOM 进入 Buffer 文本时采用的策略。
    pub bom_policy: BomPolicy,
    /// 非法 UTF-8 bytes 进入 Buffer 文本时采用的策略。
    pub invalid_utf8_policy: InvalidUtf8Policy,
    /// 原始输入开头是否包含 UTF-8 BOM。
    pub had_bom: bool,
    /// 加载过程中是否遇到并恢复过非法 UTF-8。
    pub had_invalid_utf8: bool,
    /// 加载文本中检测到的实际换行风格。
    pub line_ending_style: LineEndingStyle,
    /// 原始文本是否以换行结束，供保存和宿主体感保持使用。
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
