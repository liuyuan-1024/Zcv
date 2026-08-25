//! TrackedRange：由两个 Anchor 表达的可跟随文本变化区间。
//!
//! TrackedRange 只表达区间数学和版本推进策略，不承载 diagnostics、fold、
//! 搜索结果等业务含义。

use super::{
    Anchor, TrackedRangeCollapsePolicy, TrackedRangeInvalidationPolicy, TrackedRangeUpdate,
    TrackedRangeUpdatePolicy,
};
use crate::{
    TextResult,
    errors::AnchorError,
    position_map::{BoundarySide, MappingResult, PositionMap, Stickiness, boundary_affinity},
    transaction::DeltaEvent,
    types::{BufferVersion, TextRange},
};

/// 可跟随文本变化的区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackedRange {
    start: Anchor,
    end: Anchor,
    stickiness: Stickiness,
}

impl TrackedRange {
    pub fn new(start: Anchor, end: Anchor, stickiness: Stickiness) -> TextResult<Self> {
        if start.version() != end.version() {
            return Err(AnchorError::RangeVersionMismatch {
                start: start.version(),
                end: end.version(),
            }
            .into());
        }

        let range = TextRange::new(start.offset(), end.offset())?;

        Ok(Self::from_valid_range(start.version(), range, stickiness))
    }

    pub fn from_range(version: BufferVersion, range: TextRange, stickiness: Stickiness) -> Self {
        Self::from_valid_range(version, range, stickiness)
    }

    pub fn version(self) -> BufferVersion {
        self.start.version()
    }

    pub fn stickiness(self) -> Stickiness {
        self.stickiness
    }

    pub fn range(self) -> TextRange {
        TextRange::new(self.start.offset(), self.end.offset())
            .expect("TrackedRange 构造时已验证 start <= end")
    }

    pub fn is_empty(self) -> bool {
        self.range().is_empty()
    }

    pub fn map_through_position_map(
        self,
        new_version: BufferVersion,
        position_map: &PositionMap,
    ) -> MappingResult<Self> {
        map_range_result(
            position_map.map_old_range_with_stickiness(self.range(), self.stickiness),
            new_version,
            self.stickiness,
        )
    }

    pub fn map_through_delta_event(
        self,
        event: &DeltaEvent,
    ) -> Result<MappingResult<Self>, AnchorError> {
        self.verify_event_version(event)?;
        Ok(self.map_through_position_map(event.new_version(), event.position_map()))
    }

    /// 按失效策略映射到新版本，返回区间更新结果（供折叠等宿主查询使用）。
    pub fn map_through_position_map_with_policy(
        self,
        new_version: BufferVersion,
        position_map: &PositionMap,
        policy: TrackedRangeUpdatePolicy,
    ) -> TrackedRangeUpdate {
        let mapped = self.map_through_position_map(new_version, position_map);
        self.update_from_mapping_result(mapped, policy)
    }

    pub fn update_through_delta_event(
        &mut self,
        event: &DeltaEvent,
    ) -> Result<MappingResult<Self>, AnchorError> {
        let mapped = self.map_through_delta_event(event)?;
        *self = mapped.value();
        Ok(mapped)
    }

    fn update_from_mapping_result(
        self,
        mapped: MappingResult<Self>,
        policy: TrackedRangeUpdatePolicy,
    ) -> TrackedRangeUpdate {
        let mapped_range = mapped.value();
        if should_invalidate(mapped, policy) {
            return TrackedRangeUpdate::Invalidated {
                range: mapped_range.range(),
                version: mapped_range.version(),
            };
        }

        match mapped {
            MappingResult::Mapped(range) | MappingResult::Ambiguous(range) => {
                TrackedRangeUpdate::Mapped(range)
            }
            MappingResult::Deleted(range) => TrackedRangeUpdate::Deleted(range),
            MappingResult::Collapsed(range) => TrackedRangeUpdate::Collapsed(range),
        }
    }

    fn verify_event_version(self, event: &DeltaEvent) -> Result<(), AnchorError> {
        if self.version() != event.old_version() {
            return Err(AnchorError::VersionMismatch {
                expected: event.old_version(),
                actual: self.version(),
            });
        }

        Ok(())
    }

    fn from_valid_range(version: BufferVersion, range: TextRange, stickiness: Stickiness) -> Self {
        let start = Anchor::new(version, range.start())
            .with_affinity(boundary_affinity(stickiness, BoundarySide::Start));
        let end = Anchor::new(version, range.end())
            .with_affinity(boundary_affinity(stickiness, BoundarySide::End));

        Self {
            start,
            end,
            stickiness,
        }
    }
}

fn map_range_result(
    result: MappingResult<TextRange>,
    version: BufferVersion,
    stickiness: Stickiness,
) -> MappingResult<TrackedRange> {
    result.map(|range| TrackedRange::from_valid_range(version, range, stickiness))
}

fn should_invalidate(
    mapped: MappingResult<TrackedRange>,
    policy: TrackedRangeUpdatePolicy,
) -> bool {
    let invalidated_by_deleted_content = match policy.invalidation() {
        TrackedRangeInvalidationPolicy::Never => false,
        TrackedRangeInvalidationPolicy::WhenFullyDeleted => {
            matches!(mapped, MappingResult::Collapsed(_))
        }
        TrackedRangeInvalidationPolicy::WhenTouchedByDeletion => {
            matches!(
                mapped,
                MappingResult::Deleted(_) | MappingResult::Collapsed(_)
            )
        }
    };

    invalidated_by_deleted_content
        || (policy.collapse() == TrackedRangeCollapsePolicy::Invalidate
            && mapped.value().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnchorError, ByteOffset, TextError};

    fn b(value: usize) -> ByteOffset {
        ByteOffset::new(value)
    }

    #[test]
    fn tracked_range_should_reject_mismatched_versions() {
        let start = Anchor::new(BufferVersion::INITIAL, b(3));
        let end = Anchor::new(BufferVersion::new(1), b(6));
        let err = TrackedRange::new(start, end, Stickiness::Never).unwrap_err();
        assert!(matches!(
            err,
            TextError::Anchor(AnchorError::RangeVersionMismatch { .. })
        ));
    }
}
