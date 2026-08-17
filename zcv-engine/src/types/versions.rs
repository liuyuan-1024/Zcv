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
mod tests {
    use super::*;

    #[test]
    fn versions_and_transaction_ids_should_advance_until_overflow_boundary() {
        assert_eq!(BufferVersion::INITIAL.next(), Some(BufferVersion::new(1)));
        assert_eq!(TransactionId::INITIAL.next(), Some(TransactionId::new(1)));
        assert_eq!(BufferVersion::new(u64::MAX).next(), None);
        assert_eq!(TransactionId::new(u64::MAX).next(), None);
    }
}
