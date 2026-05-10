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
    /// 加载后进入 Buffer 的 UTF-8 文本字节数（已应用 BOM 策略后的有效字节）。
    pub loaded_byte_size: usize,
    /// 按 `LargeFilePolicy::large_file_threshold_bytes` 判定是否为大文件的快照值。
    pub is_large: bool,
    /// 加载文本中最长一行的字符数（不含行尾换行符）。
    pub longest_line_chars: usize,
    /// 按 `LargeFilePolicy::long_line_threshold_chars` 判定是否含超长行的快照值。
    pub has_long_line: bool,
}

impl LoadedTextInfo {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        encoding: TextEncoding,
        bom_policy: BomPolicy,
        invalid_utf8_policy: InvalidUtf8Policy,
        had_bom: bool,
        had_invalid_utf8: bool,
        line_ending_style: LineEndingStyle,
        has_final_newline: bool,
        loaded_byte_size: usize,
        is_large: bool,
        longest_line_chars: usize,
        has_long_line: bool,
    ) -> Self {
        Self {
            encoding,
            bom_policy,
            invalid_utf8_policy,
            had_bom,
            had_invalid_utf8,
            line_ending_style,
            has_final_newline,
            loaded_byte_size,
            is_large,
            longest_line_chars,
            has_long_line,
        }
    }
}
