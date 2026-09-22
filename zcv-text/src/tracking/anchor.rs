//! Anchor：绑定 `BufferVersion` 的稳定位置标记。
//!
//! 所有文本更新都沿同一版本化坐标链推进；
//! `resolve_in` 通过 Buffer 的不衰减坐标索引重新定位，不依赖会被预算裁剪的带文本编辑日志。

use std::ops::Range;

use crate::{
    errors::AnchorError,
    position_map::{Affinity, MappingResult, PositionMap},
    transaction::DeltaEvent,
    types::{BufferVersion, ByteOffset, TextRange},
};

/// 绑定 BufferVersion 的稳定位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Anchor {
    version: BufferVersion,
    offset: ByteOffset,
    affinity: Affinity,
}

impl Anchor {
    pub fn new(version: BufferVersion, offset: ByteOffset) -> Self {
        Self {
            version,
            offset,
            affinity: Affinity::default(),
        }
    }

    pub fn with_affinity(mut self, affinity: Affinity) -> Self {
        self.affinity = affinity;
        self
    }

    pub fn version(self) -> BufferVersion {
        self.version
    }

    pub fn offset(self) -> ByteOffset {
        self.offset
    }

    pub fn affinity(self) -> Affinity {
        self.affinity
    }

    /// 创建一个不吸收边界插入的锚点范围。
    ///
    /// 起点贴在插入内容之后，终点贴在插入内容之前；
    /// 适合折叠、diff hunk 等只跟随原有文本而不扩张的范围。
    pub fn range_inside(version: BufferVersion, range: TextRange) -> Range<Self> {
        Self::new(version, range.start()).with_affinity(Affinity::After)
            ..Self::new(version, range.end()).with_affinity(Affinity::Before)
    }

    /// 创建一个吸收边界插入的锚点范围。
    ///
    /// 起点贴在插入内容之前，终点贴在插入内容之后。
    /// 适合自动闭合对这类需要把内部新输入继续纳入范围的标记。
    pub fn range_outside(version: BufferVersion, range: TextRange) -> Range<Self> {
        Self::new(version, range.start()).with_affinity(Affinity::Before)
            ..Self::new(version, range.end()).with_affinity(Affinity::After)
    }

    /// 把锚点解析到目标快照的当前坐标。
    ///
    /// 目标快照更旧返回 [`AnchorError::TargetBeforeSource`]。
    /// 调用方必须显式处理失败，不得把锚点原始偏移当成目标快照坐标。
    pub fn resolve_in(&self, snapshot: &crate::Snapshot) -> crate::TextResult<ByteOffset> {
        let map = snapshot.position_map_since(self.version)?;
        Ok(map
            .map_old_position_with_affinity(self.offset, self.affinity)
            .value())
    }

    /// 用一次显式增量把锚点推进到新版本。
    pub fn map_through_position_map(
        self,
        new_version: BufferVersion,
        position_map: &PositionMap,
    ) -> MappingResult<Self> {
        self.advance(new_version, position_map)
    }

    pub fn map_through_delta_event(
        self,
        event: &DeltaEvent,
    ) -> Result<MappingResult<Self>, AnchorError> {
        self.verify_event_version(event)?;
        Ok(self.advance(event.new_version(), event.position_map()))
    }

    fn advance(
        self,
        new_version: BufferVersion,
        position_map: &PositionMap,
    ) -> MappingResult<Self> {
        position_map
            .map_old_position_with_affinity(self.offset, self.affinity)
            .map(|offset| Anchor::new(new_version, offset).with_affinity(self.affinity))
    }

    fn verify_event_version(self, event: &DeltaEvent) -> Result<(), AnchorError> {
        if self.version != event.old_version() {
            return Err(AnchorError::VersionMismatch {
                expected: event.old_version(),
                actual: self.version,
            });
        }

        Ok(())
    }
}

impl Default for Anchor {
    fn default() -> Self {
        Self::new(BufferVersion::INITIAL, ByteOffset::ZERO)
    }
}

#[cfg(test)]
#[path = "test/anchor_tests.rs"]
mod tests;
