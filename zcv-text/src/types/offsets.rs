//! 一维偏移强类型：区分 UTF-8 byte（文本内核核心）、Unicode scalar 和 UTF-16 code unit 坐标。
//!
//! **坐标系唯一真理**：文本内核内部以 `ByteOffset(usize)` 为核心位置；
//! `CharOffset` / `Utf16Offset` 仅作为边界处的"投影"类型暴露给外部协议。

/// 字节偏移量 —— 文本内核核心位置类型。
///
/// UTF-8 文本存储中的物理坐标。**文本内核内部所有 Edit / TextRange / PositionMap /
/// ChangeSet / Storage / Anchor 都以 `ByteOffset` 为单一真理**。
/// `CharOffset` / `Line` / `LogicalColumn` / `Utf16Offset` 是边界投影类型。
///
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ByteOffset(usize);

impl ByteOffset {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }

    pub fn checked_add(self, rhs: usize) -> Option<Self> {
        self.0.checked_add(rhs).map(Self)
    }

    pub fn checked_sub(self, rhs: usize) -> Option<Self> {
        self.0.checked_sub(rhs).map(Self)
    }

    pub fn saturating_add(self, rhs: usize) -> Self {
        Self(self.0.saturating_add(rhs))
    }

    pub fn saturating_sub(self, rhs: usize) -> Self {
        Self(self.0.saturating_sub(rhs))
    }
}

impl core::fmt::Display for ByteOffset {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 字符偏移量 —— 边界投影类型。
///
/// 按 Unicode Scalar Value 计数。**仅在公共 API 边界**（如 LSP、外部协议、UI）
/// 使用；文本内核内部不以 `CharOffset` 为位置坐标，必须经存储后端的字节↔字符
/// 投影函数转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CharOffset(usize);

impl CharOffset {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }

    pub fn checked_add(self, rhs: usize) -> Option<Self> {
        self.0.checked_add(rhs).map(Self)
    }

    pub fn checked_sub(self, rhs: usize) -> Option<Self> {
        self.0.checked_sub(rhs).map(Self)
    }
}

impl core::fmt::Display for CharOffset {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// UTF-16 偏移量 —— 边界投影类型。
///
/// 用于外部协议交互（例如 LSP）。文本内核内部不以 `Utf16Offset` 为位置坐标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Utf16Offset(usize);

impl Utf16Offset {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

impl core::fmt::Display for Utf16Offset {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(value: usize) -> ByteOffset {
        ByteOffset::new(value)
    }

    #[test]
    fn offsets_checked_and_saturating_arithmetic_should_not_panic_at_usize_edges() {
        assert_eq!(b(4).checked_add(3), Some(b(7)));
        assert_eq!(b(4).checked_sub(5), None);
        assert_eq!(ByteOffset::new(usize::MAX).checked_add(1), None);
        assert_eq!(b(4).saturating_sub(9), ByteOffset::ZERO);
        assert_eq!(
            ByteOffset::new(usize::MAX).saturating_add(1),
            ByteOffset::new(usize::MAX)
        );
        assert_eq!(CharOffset::new(4).checked_sub(5), None);
    }
}
