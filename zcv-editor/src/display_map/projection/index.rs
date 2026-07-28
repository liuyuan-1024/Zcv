//! Editor projected line 索引：投影空间中的行号强类型，与逻辑行 `Line` 区分。

/// Projection 中投影行的 0-indexed 索引。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ProjectedLineIndex(usize);

impl ProjectedLineIndex {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}
