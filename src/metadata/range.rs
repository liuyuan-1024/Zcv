//! 单条 MetadataRange：把外部 payload 绑定到可跟随文本变化的 TrackedRange。
//!
//! 本文件不解释 payload 业务含义，只维护 range、版本、stickiness 和 update policy 的组合。

use crate::{
    errors::AnchorError,
    position_map::Stickiness,
    tracking::{TrackedRange, TrackedRangeUpdate, TrackedRangeUpdatePolicy},
    transaction::DeltaEvent,
    types::{BufferVersion, TextRange},
};

use super::MetadataRangeId;

/// 单条外部 metadata 与其可跟随文本区间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataRange<T> {
    id: MetadataRangeId,
    tracked_range: TrackedRange,
    update_policy: TrackedRangeUpdatePolicy,
    metadata: T,
}

impl<T> MetadataRange<T> {
    pub fn new(
        id: MetadataRangeId,
        version: BufferVersion,
        range: TextRange,
        stickiness: Stickiness,
        metadata: T,
    ) -> Self {
        Self::with_policy(
            id,
            version,
            range,
            stickiness,
            TrackedRangeUpdatePolicy::default(),
            metadata,
        )
    }

    pub fn with_policy(
        id: MetadataRangeId,
        version: BufferVersion,
        range: TextRange,
        stickiness: Stickiness,
        update_policy: TrackedRangeUpdatePolicy,
        metadata: T,
    ) -> Self {
        Self {
            id,
            tracked_range: TrackedRange::from_range(version, range, stickiness),
            update_policy,
            metadata,
        }
    }

    pub fn id(&self) -> MetadataRangeId {
        self.id
    }

    pub fn version(&self) -> BufferVersion {
        self.tracked_range.version()
    }

    pub fn range(&self) -> TextRange {
        self.tracked_range.range()
    }

    pub fn tracked_range(&self) -> TrackedRange {
        self.tracked_range
    }

    pub fn stickiness(&self) -> Stickiness {
        self.tracked_range.stickiness()
    }

    pub fn update_policy(&self) -> TrackedRangeUpdatePolicy {
        self.update_policy
    }

    pub fn metadata(&self) -> &T {
        &self.metadata
    }

    pub fn metadata_mut(&mut self) -> &mut T {
        &mut self.metadata
    }

    pub fn into_metadata(self) -> T {
        self.metadata
    }

    pub fn map_through_delta_event(
        &self,
        event: &DeltaEvent,
    ) -> Result<TrackedRangeUpdate, AnchorError> {
        self.tracked_range
            .map_through_delta_event_with_policy(event, self.update_policy)
    }

    pub(super) fn set_tracked_range(&mut self, tracked_range: TrackedRange) {
        self.tracked_range = tracked_range;
    }
}
