//! 版本与 ID 强类型：隔离 Buffer 版本、稳定身份和事务身份。
//!
//! 这些值只表达单调编号，不承载文件路径、时间戳或外部项目 ID。

use std::sync::atomic::{AtomicU64, Ordering};

/// Buffer 的稳定身份。
///
/// 它对应 Zed 的 `remote_id`：本地模式同样必须为没有文件路径的 Buffer 提供可比较、可排序的身份，不能把所有匿名 Buffer 归并为空路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferId(u64);

impl BufferId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub(crate) fn next_local() -> Self {
        static NEXT_BUFFER_ID: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_BUFFER_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl std::fmt::Display for BufferId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
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

#[cfg(test)]
#[path = "test/versions_tests.rs"]
mod tests;
