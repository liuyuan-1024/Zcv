//! Metadata range 身份：为单个 MetadataLayer 内的 range 分配稳定递增 ID。
//!
//! ID 只在 layer 生命周期内稳定，不跨 layer、不跨替换批次表达全局身份。
//! 构造与初始值是引擎内部分配契约，不向 crate 外部暴露——外部代码只能持有 layer 颁发的 ID
//! 并通过 `get()` 读取数值用于日志或调试。

/// MetadataRange 在单个 MetadataLayer 内的稳定身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MetadataRangeId(u64);

impl MetadataRangeId {
    pub(crate) const INITIAL: Self = Self(0);

    pub const fn get(self) -> u64 {
        self.0
    }

    pub(super) fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}
