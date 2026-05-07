//! 一维偏移强类型：区分 UTF-8 byte、Unicode scalar 和 UTF-16 code unit 坐标。
//!
//! M3.5 后核心编辑 API 使用 CharOffset，其他偏移保留给编码和协议边界。

/// 字节偏移量。
///
/// 这是 UTF-8 文本存储结构中的物理坐标。M3.5 起，编辑 API 不再使用
/// ByteOffset；它保留给文件字节、编码边界和后续外部协议适配层。
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
/// code unit 偏移量。M3.5 起，这是编辑引擎内部和 public 编辑 API 的主坐标。
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
/// 主要用于 LSP 等外部协议交互。
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
