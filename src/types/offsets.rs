//! 一维偏移强类型：区分 UTF-8 byte、Unicode scalar 和 UTF-16 code unit 坐标。
//!
//! 编辑 API 一律使用 `CharOffset`；`ByteOffset` / `Utf16Offset` 保留给编码和外部协议边界。

/// 字节偏移量。
///
/// UTF-8 文本存储中的物理坐标，仅用于文件字节、编码探测和外部协议适配。
/// 编辑入口不接受 `ByteOffset`。
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
}

/// 字符偏移量。
///
/// 按 Unicode Scalar Value 计数，不等同于字节偏移量，也不等同于 UTF-16
/// code unit 偏移量；引擎内部与 public 编辑 API 一律以 `CharOffset` 为主坐标。
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

/// UTF-16 偏移量。
///
/// 用于外部协议交互（例如 LSP）。
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
