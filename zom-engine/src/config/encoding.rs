//! 外部 bytes 进入 Buffer 前后的编码恢复策略。
//!
//! 这里只保存策略选择；实际 UTF-8 校验、BOM 剥离和替换发生在 Buffer 加载边界。

use crate::{BomPolicy, InvalidUtf8Policy};

/// 外部文本进入 Buffer 时的编码与恢复策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodingConfig {
    /// UTF-8 BOM 是剥离并记录，还是作为 U+FEFF 保留到 Buffer 文本中。
    pub bom: BomPolicy,
    /// 非法 UTF-8 bytes 是拒绝加载，还是用 replacement character 恢复为可编辑文本。
    pub invalid_utf8: InvalidUtf8Policy,
}

impl EncodingConfig {
    pub const fn new(bom: BomPolicy, invalid_utf8: InvalidUtf8Policy) -> Self {
        Self { bom, invalid_utf8 }
    }
}

impl Default for EncodingConfig {
    fn default() -> Self {
        Self {
            bom: BomPolicy::Strip,
            invalid_utf8: InvalidUtf8Policy::Reject,
        }
    }
}
