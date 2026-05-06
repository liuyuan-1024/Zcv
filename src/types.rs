//! 领域强类型：集中定义编辑引擎的坐标、位置、范围、版本和事务标识。
//!
//! 本文件是 public API 的语义地基，只表达类型不变量和轻量运算，不绑定 Buffer、存储或历史实现。

use std::path::{Path, PathBuf};

use crate::CoordinateError;

// ==========================================
// 1. 基础坐标体系 (1D Offsets)
// ==========================================

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

/// UTF-16 行列位置。
///
/// 主要用于 LSP 等使用 UTF-16 code unit 作为行内坐标的外部协议。
/// `line` 仍然是 0-indexed 逻辑行号，`character` 是该行内 UTF-16 code unit 偏移。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Utf16Position {
    pub line: Line,
    pub character: Utf16Offset,
}

impl Utf16Position {
    pub const ZERO: Self = Self {
        line: Line::ZERO,
        character: Utf16Offset::ZERO,
    };

    pub const fn new(line: Line, character: Utf16Offset) -> Self {
        Self { line, character }
    }

    pub const fn line(self) -> Line {
        self.line
    }

    pub const fn character(self) -> Utf16Offset {
        self.character
    }
}

/// 文件中检测到的换行风格。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LineEndingStyle {
    /// 文本中没有出现换行符。
    #[default]
    None,
    /// 只检测到 LF (`\n`)。
    Lf,
    /// 只检测到 CRLF (`\r\n`)。
    Crlf,
    /// 同时出现多种换行风格，或出现孤立 CR。
    Mixed,
}

// ==========================================
// 2. 行列逻辑坐标体系 (2D Coordinates)
// ==========================================

/// 逻辑行号，0-indexed。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Line(usize);

impl Line {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

/// 逻辑列号，0-indexed。
///
/// M3.5 起，逻辑列按 Unicode Scalar Value 计数，与 CharOffset 的行内单位一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct LogicalColumn(usize);

impl LogicalColumn {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

/// 视觉列号，0-indexed。
///
/// 表示考虑 Tab 展开、CJK 宽度、emoji 宽度等策略后的显示列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DisplayColumn(usize);

impl DisplayColumn {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

/// 逻辑文本位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Position {
    pub line: Line,
    pub column: LogicalColumn,
}

impl Position {
    pub const ZERO: Self = Self {
        line: Line::ZERO,
        column: LogicalColumn::ZERO,
    };

    pub const fn new(line: Line, column: LogicalColumn) -> Self {
        Self { line, column }
    }

    pub const fn line(self) -> Line {
        self.line
    }

    pub const fn column(self) -> LogicalColumn {
        self.column
    }
}

// ==========================================
// 3. 区间体系
// ==========================================

/// 文本区间。
///
/// M3.5 起，TextRange 由 CharOffset 构成，满足 `start <= end`。
/// 这意味着 TextRange 是编辑语义区间，不再是 UTF-8 字节区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextRange {
    start: CharOffset,
    end: CharOffset,
}

impl TextRange {
    /// 创建文本区间。
    ///
    /// 该构造函数会校验 `start <= end`，避免在公共 API 边界 panic。
    pub fn new(start: CharOffset, end: CharOffset) -> Result<Self, CoordinateError> {
        if start > end {
            return Err(CoordinateError::InvalidRange { start, end });
        }

        Ok(Self { start, end })
    }

    pub const fn start(self) -> CharOffset {
        self.start
    }

    pub const fn end(self) -> CharOffset {
        self.end
    }

    pub fn len(self) -> usize {
        self.end.get() - self.start.get()
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

// ==========================================
// 4. 版本与事务追踪
// ==========================================

/// Buffer 身份。
///
/// M7A 起，Buffer 不再只是文本容器，也需要能被宿主作为文档对象追踪。
/// `BufferId` 只表达引擎内身份，不等同于文件路径、URI 或外部项目索引 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferId(u64);

impl BufferId {
    pub const INITIAL: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Default for BufferId {
    fn default() -> Self {
        Self::INITIAL
    }
}

/// Buffer 的单调递增版本号。
///
/// 每次事务成功提交后递增。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferVersion(u64);

impl BufferVersion {
    /// 初值
    pub const INITIAL: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

impl Default for BufferVersion {
    fn default() -> Self {
        Self::INITIAL
    }
}

/// 事务 ID。
///
/// 用于标识一次事务提交，通常单调递增。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TransactionId(u64);

impl TransactionId {
    /// 初值
    pub const INITIAL: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// Buffer 的来源 / 生命周期类型。
///
/// M7A 只记录身份边界：文件、URI、未命名文档与临时草稿。文件加载、编码探测、
/// reload 和保存输出属于 M7C/M7D，不在这里承诺。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BufferKind {
    File { path: PathBuf },
    Uri { uri: String },
    Untitled,
    Scratch,
}

impl BufferKind {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::File { path: path.into() }
    }

    pub fn uri(uri: impl Into<String>) -> Self {
        Self::Uri { uri: uri.into() }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File { path } => Some(path.as_path()),
            Self::Uri { .. } | Self::Untitled | Self::Scratch => None,
        }
    }

    pub fn uri_str(&self) -> Option<&str> {
        match self {
            Self::Uri { uri } => Some(uri.as_str()),
            Self::File { .. } | Self::Untitled | Self::Scratch => None,
        }
    }

    pub fn is_temporary(&self) -> bool {
        matches!(self, Self::Untitled | Self::Scratch)
    }
}

impl Default for BufferKind {
    fn default() -> Self {
        Self::Untitled
    }
}

/// Buffer 当前对宿主可见的生命周期状态。
///
/// M7A 先提供 Clean / Dirty / ReadOnly 的真实状态判断；Loading、Reloading、
/// Conflict 是后续文件边界与 reload 流程的公共状态词汇。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BufferState {
    Clean,
    Dirty,
    ReadOnly,
    Loading,
    Reloading,
    Conflict,
}
