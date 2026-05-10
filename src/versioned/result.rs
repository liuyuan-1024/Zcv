//! M14A `VersionedResult<T>`：把任意 payload 与 `BufferVersion` 绑定的通用载体。
//!
//! 本模块只表达版本绑定、过期判断、过期丢弃和通过 `PositionMap` 尝试 remap 的边界；
//! 不携带任何业务 payload 语义，具体的 remap 行为由调用方在闭包里完成。

use crate::{
    EngineResult, errors::VersionedResultError, position_map::PositionMap, transaction::DeltaEvent,
    types::BufferVersion,
};

/// 与 `BufferVersion` 绑定的泛型结果载体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VersionedResult<T> {
    version: BufferVersion,
    value: T,
}

impl<T> VersionedResult<T> {
    /// 构造一个版本绑定结果。
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

    /// 可变 payload。
    pub fn value_mut(&mut self) -> &mut T {
        &mut self.value
    }

    /// 拿走 payload，丢弃版本绑定。
    pub fn into_value(self) -> T {
        self.value
    }

    /// 拆解为版本与 payload。
    pub fn into_parts(self) -> (BufferVersion, T) {
        (self.version, self.value)
    }

    /// 不改版本地变换 payload。
    pub fn map<U, F>(self, f: F) -> VersionedResult<U>
    where
        F: FnOnce(T) -> U,
    {
        VersionedResult {
            version: self.version,
            value: f(self.value),
        }
    }

    /// 当前结果是否相对 `current` 版本已过期。
    pub fn is_stale(&self, current: BufferVersion) -> bool {
        self.version != current
    }

    /// 过期时丢弃，未过期时保留。
    pub fn discard_if_stale(self, current: BufferVersion) -> Option<Self> {
        if self.is_stale(current) {
            None
        } else {
            Some(self)
        }
    }

    /// 通过一次 `DeltaEvent` 把 payload 推进到新版本。
    ///
    /// `event.old_version` 必须与当前结果版本一致，否则原子拒绝、不调用 `remap`。
    /// 闭包接收 payload by value 与 `&PositionMap`，返回新 payload 或失败原因。
    pub fn try_remap<F>(self, event: &DeltaEvent, remap: F) -> EngineResult<VersionedResult<T>>
    where
        F: FnOnce(T, &PositionMap) -> Result<T, VersionedResultError>,
    {
        if event.old_version != self.version {
            return Err(VersionedResultError::VersionMismatch {
                expected: self.version,
                actual: event.old_version,
            }
            .into());
        }

        let new_value = remap(self.value, &event.position_map)?;
        Ok(VersionedResult::new(event.new_version, new_value))
    }

    /// 显式给定 `PositionMap` 与目标版本的低层 remap 入口。
    ///
    /// 调用方需自行保证 `position_map` 真的来自旧版本到 `new_version` 的推进。
    pub fn try_remap_with<F>(
        self,
        position_map: &PositionMap,
        new_version: BufferVersion,
        remap: F,
    ) -> EngineResult<VersionedResult<T>>
    where
        F: FnOnce(T, &PositionMap) -> Result<T, VersionedResultError>,
    {
        let new_value = remap(self.value, position_map)?;
        Ok(VersionedResult::new(new_version, new_value))
    }
}
