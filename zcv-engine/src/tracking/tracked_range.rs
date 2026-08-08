//! TrackedRange：由两个 Anchor 表达的可跟随文本变化区间。
//!
//! TrackedRange 只表达区间数学和版本推进策略，不承载 diagnostics、fold、
//! 搜索结果等业务含义。

use crate::{
    EngineResult,
    errors::AnchorError,
    position_map::{MappingResult, PositionMap, Stickiness},
    transaction::DeltaEvent,
    types::{BufferVersion, TextRange},
};

use super::{
    Anchor, TrackedRangeCollapsePolicy, TrackedRangeInvalidationPolicy, TrackedRangeUpdate,
    TrackedRangeUpdatePolicy,
};

/// 可跟随文本变化的区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackedRange {
    start: Anchor,
    end: Anchor,
    stickiness: Stickiness,
}

impl TrackedRange {
    pub fn new(start: Anchor, end: Anchor, stickiness: Stickiness) -> EngineResult<Self> {
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

    pub fn start_anchor(self) -> Anchor {
        self.start
    }

    pub fn end_anchor(self) -> Anchor {
        self.end
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

    pub fn with_stickiness(self, stickiness: Stickiness) -> Self {
        Self::from_valid_range(self.version(), self.range(), stickiness)
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

    pub fn map_through_delta_event_with_policy(
        self,
        event: &DeltaEvent,
        policy: TrackedRangeUpdatePolicy,
    ) -> Result<TrackedRangeUpdate, AnchorError> {
        self.verify_event_version(event)?;
        Ok(self.map_through_position_map_with_policy(
            event.new_version(),
            event.position_map(),
            policy,
        ))
    }

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
        let start = Anchor::new(version, range.start()).with_affinity(
            crate::position_map::boundary_affinity(
                stickiness,
                crate::position_map::BoundarySide::Start,
            ),
        );
        let end = Anchor::new(version, range.end()).with_affinity(
            crate::position_map::boundary_affinity(
                stickiness,
                crate::position_map::BoundarySide::End,
            ),
        );

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
    use crate::{
        AnchorError, ChangeSet, Delta, Edit, EditList, EngineError, PositionMap, TransactionId,
        TransactionSource,
    };

    fn b(value: usize) -> crate::ByteOffset {
        crate::ByteOffset::new(value)
    }

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(b(start), b(end)).unwrap()
    }

    fn event_for_edits(
        old_version: BufferVersion,
        new_version: BufferVersion,
        edits: Vec<Edit>,
    ) -> DeltaEvent {
        let edit_list = EditList::new(edits).unwrap();
        let delta = Delta::new(old_version, new_version, edit_list.clone());
        let changeset = ChangeSet::from_edit_list(&edit_list);
        let position_map = PositionMap::from_edits(edit_list.as_slice());

        DeltaEvent::new(
            TransactionId::INITIAL,
            TransactionSource::Programmatic,
            delta,
            changeset,
            position_map,
        )
    }

    #[test]
    fn tracked_range_should_reject_mismatched_versions_and_invalidate_when_policy_matches_deletion()
    {
        let start = Anchor::new(BufferVersion::INITIAL, b(3));
        let end = Anchor::new(BufferVersion::new(1), b(6));
        let err = TrackedRange::new(start, end, Stickiness::Never).unwrap_err();
        assert!(matches!(
            err,
            EngineError::Anchor(AnchorError::RangeVersionMismatch { .. })
        ));

        let tracked =
            TrackedRange::from_range(BufferVersion::INITIAL, range(1, 4), Stickiness::Never);
        let event = event_for_edits(
            BufferVersion::INITIAL,
            BufferVersion::new(1),
            vec![Edit::delete(range(1, 4))],
        );
        let update = tracked
            .map_through_delta_event_with_policy(
                &event,
                TrackedRangeUpdatePolicy::invalidate_when_fully_deleted(),
            )
            .unwrap();

        assert!(matches!(
            update,
            TrackedRangeUpdate::Invalidated { range, version }
                if range == TextRange::new(b(1), b(1)).unwrap() && version == event.new_version()
        ));
    }
}
