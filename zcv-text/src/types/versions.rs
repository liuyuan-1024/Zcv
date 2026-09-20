//! 版本与 ID 强类型：隔离 Buffer 版本和事务身份。
//!
//! 这些值只表达单调编号，不承载文件路径、时间戳或外部项目 ID。

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

/// Buffer 内容代际：标识一段连续可映射的版本历史。
///
/// 普通版本推进不改变代际；reset / 外部基线替换会开启新代际，
/// 使替换前的锚点无法再被当作普通编辑继续映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferGeneration(BufferVersion);

impl BufferGeneration {
    /// 初始代际：Buffer 创建后、首次基线替换前的版本空间。
    pub const INITIAL: Self = Self(BufferVersion::INITIAL);

    pub const fn new(version: BufferVersion) -> Self {
        Self(version)
    }

    /// 该代际起始的版本。首个提交属于本代际时等于该提交的 new_version。
    pub const fn version(self) -> BufferVersion {
        self.0
    }
}

impl Default for BufferGeneration {
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

#[cfg(test)]
#[path = "test/versions_tests.rs"]
mod tests;
