//! Fold range 身份：为单个 FoldSet 内的 fold 分配稳定递增 ID。
//!
//! ID 只在 FoldSet 生命周期内稳定，不跨 FoldSet 表达全局身份。

/// FoldRange 在单个 FoldSet 内的稳定身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FoldRangeId(u64);

impl FoldRangeId {
    pub const INITIAL: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub(super) fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}
