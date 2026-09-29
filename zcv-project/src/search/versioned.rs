//! `VersionedResult<T>`：把任意 payload 与 `BufferVersion` 绑定的通用载体。
//!
//! 本模块只表达版本绑定与过期判断，不携带任何业务 payload 语义。

use zcv_text::BufferVersion;

/// 与 `BufferVersion` 绑定的泛型结果载体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct VersionedResult<T> {
    version: BufferVersion,
    value: T,
}

impl<T> VersionedResult<T> {
    /// 绑定结果到 `BufferVersion`，用于消费侧校验版本一致性。
    pub const fn new(version: BufferVersion, value: T) -> Self {
        Self { version, value }
    }

    /// 结果绑定的 BufferVersion。
    pub fn version(&self) -> BufferVersion {
        self.version
    }

    /// 只读 payload。
    pub fn value(&self) -> &T {
        &self.value
    }

    /// 当前结果是否相对 `current` 版本已过期。
    pub fn is_stale(&self, current: BufferVersion) -> bool {
        self.version != current
    }
}
