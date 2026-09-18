//! `VersionedResult<T>`：把任意 payload 与 `BufferVersion` 绑定的通用载体。
//!
//! 本模块只表达版本绑定、过期判断和通过 `PositionMap` 尝试 remap 的边界；
//! 不携带任何业务 payload 语义，具体的 remap 行为由调用方在闭包里完成。

use zcv_text::{BufferVersion, DeltaEvent, PositionMap};

use super::error::{SearchError, VersionedResultError};

/// 与 `BufferVersion` 绑定的泛型结果载体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VersionedResult<T> {
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

    /// 通过一次 `DeltaEvent` 把 payload 推进到新版本。
    ///
    /// `event.old_version()` 必须与当前结果版本一致，否则原子拒绝、不调用 `remap`。
    pub fn try_remap<F>(
        self,
        event: &DeltaEvent,
        remap: F,
    ) -> Result<VersionedResult<T>, SearchError>
    where
        F: FnOnce(T, &PositionMap) -> Result<T, VersionedResultError>,
    {
        if event.old_version() != self.version {
            return Err(VersionedResultError::VersionMismatch {
                expected: self.version,
                actual: event.old_version(),
            }
            .into());
        }

        let new_value = remap(self.value, event.position_map())?;
        Ok(VersionedResult::new(event.new_version(), new_value))
    }
}
