//! Anchor：绑定 `BufferGeneration` 与 `BufferVersion` 的稳定位置标记。
//!
//! 普通版本推进不改变代际；`resolve_in` 通过 Buffer 的不衰减坐标索引重新定位，
//! 不依赖会被预算裁剪的带文本编辑日志。reset / 基线替换会开启新代际，
//! 旧代际锚点解析显式失败，不会被钳到邻近坐标。

use std::ops::Range;

use crate::{
    errors::AnchorError,
    position_map::{Affinity, MappingResult, PositionMap},
    transaction::DeltaEvent,
    types::{BufferGeneration, BufferVersion, ByteOffset, TextRange},
};

/// 绑定内容代际与 BufferVersion 的稳定位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Anchor {
    generation: BufferGeneration,
    version: BufferVersion,
    offset: ByteOffset,
    affinity: Affinity,
}

impl Anchor {
    pub fn new(generation: BufferGeneration, version: BufferVersion, offset: ByteOffset) -> Self {
        Self {
            generation,
            version,
            offset,
            affinity: Affinity::default(),
        }
    }

    pub fn with_affinity(mut self, affinity: Affinity) -> Self {
        self.affinity = affinity;
        self
    }

    pub fn generation(self) -> BufferGeneration {
        self.generation
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
    pub fn range_inside(
        generation: BufferGeneration,
        version: BufferVersion,
        range: TextRange,
    ) -> Range<Self> {
        Self::new(generation, version, range.start()).with_affinity(Affinity::After)
            ..Self::new(generation, version, range.end()).with_affinity(Affinity::Before)
    }

    /// 创建一个吸收边界插入的锚点范围。
    ///
    /// 起点贴在插入内容之前，终点贴在插入内容之后。
    /// 适合自动闭合对这类需要把内部新输入继续纳入范围的标记。
    pub fn range_outside(
        generation: BufferGeneration,
        version: BufferVersion,
        range: TextRange,
    ) -> Range<Self> {
        Self::new(generation, version, range.start()).with_affinity(Affinity::Before)
            ..Self::new(generation, version, range.end()).with_affinity(Affinity::After)
    }

    /// 把锚点解析到目标快照的当前坐标。
    ///
    /// 目标快照更旧返回 [`AnchorError::TargetBeforeSource`]；锚点代际已被
    /// reset / 基线替换淘汰返回 [`AnchorError::GenerationMismatch`]。
    /// 调用方必须显式处理失败，不得把锚点原始偏移当成目标快照坐标。
    pub fn resolve_in(&self, snapshot: &crate::Snapshot) -> crate::TextResult<ByteOffset> {
        let map = snapshot.position_map_since(self.generation, self.version)?;
        Ok(map
            .map_old_position_with_affinity(self.offset, self.affinity)
            .value())
    }

    /// 跨代际显式重锚到目标快照。
    ///
    /// 用于 reset / 基线替换后恢复调用方自己的稳定表示（例如外部 reload 后的光标）；
    /// 这不是普通版本推进，只有明确知晓重置语义的调用方才应使用。
    pub fn rebase_across_generations(self, snapshot: &crate::Snapshot) -> crate::TextResult<Self> {
        let map = snapshot.position_map_for_rebase(self.version)?;
        Ok(Self::new(
            snapshot.generation(),
            snapshot.version(),
            map.map_old_position_with_affinity(self.offset, self.affinity)
                .value(),
        )
        .with_affinity(self.affinity))
    }

    /// 用一次显式增量把锚点推进到新版本，保留当前内容代际。
    ///
    /// 调用方保证该增量属于同一代际（没有发生 reset / 基线替换）；
    /// 跨代际的推进必须走 [`Anchor::map_through_delta_event`]，由它切换代际。
    pub fn map_through_position_map(
        self,
        new_version: BufferVersion,
        position_map: &PositionMap,
    ) -> MappingResult<Self> {
        self.advance(new_version, self.generation, position_map)
    }

    pub fn map_through_delta_event(
        self,
        event: &DeltaEvent,
    ) -> Result<MappingResult<Self>, AnchorError> {
        self.verify_event_version(event)?;
        // reset 会开启新代际；只有显式跟随 reset 事件的锚点才切换到新代际。
        let generation = if event.requires_reset() {
            BufferGeneration::new(event.new_version())
        } else {
            self.generation
        };
        Ok(self.advance(event.new_version(), generation, event.position_map()))
    }

    pub fn update_through_delta_event(
        &mut self,
        event: &DeltaEvent,
    ) -> Result<MappingResult<Self>, AnchorError> {
        let mapped = self.map_through_delta_event(event)?;
        *self = mapped.value();
        Ok(mapped)
    }

    fn advance(
        self,
        new_version: BufferVersion,
        generation: BufferGeneration,
        position_map: &PositionMap,
    ) -> MappingResult<Self> {
        position_map
            .map_old_position_with_affinity(self.offset, self.affinity)
            .map(|offset| Anchor::new(generation, new_version, offset).with_affinity(self.affinity))
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
        Self::new(
            BufferGeneration::INITIAL,
            BufferVersion::INITIAL,
            ByteOffset::ZERO,
        )
    }
}

#[cfg(test)]
#[path = "test/anchor_tests.rs"]
mod tests;
