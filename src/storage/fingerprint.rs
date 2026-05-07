//! 文本指纹：为保存点和缓存提供低成本内容差异筛选。
//!
//! 指纹不是相等性证明；命中时仍由 RopeyStorage 回到精确文本比较。

use crate::CharOffset;

/// 文本内容指纹，用于保存点、缓存和低成本脏状态判断。
///
/// 指纹只作为快速分流；需要证明内容相等时仍应回到存储层做精确比较。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TextFingerprint {
    len_bytes: usize,
    len_chars: CharOffset,
    hash: u64,
}

impl TextFingerprint {
    pub(crate) fn new(len_bytes: usize, len_chars: CharOffset, hash: u64) -> Self {
        Self {
            len_bytes,
            len_chars,
            hash,
        }
    }
}
