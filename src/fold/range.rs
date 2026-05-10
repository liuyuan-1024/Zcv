//! 单条 FoldRange：把折叠区间绑定到可跟随文本变化的 TrackedRange。
//!
//! Fold 只表达「这一段文本被折叠」事实；占位符样式、绘制和投影由 `projection` 模块承载。
//! 折叠区间使用 `Stickiness::Never`：用户显式折叠的范围不应该在两端插入时主动扩张。

use crate::{
    errors::AnchorError,
    position_map::Stickiness,
    tracking::{TrackedRange, TrackedRangeUpdate, TrackedRangeUpdatePolicy},
    transaction::DeltaEvent,
    types::{BufferVersion, TextRange},
};

use super::FoldRangeId;

/// 单条折叠区间与其可跟随文本的内部 TrackedRange。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FoldRange {
    id: FoldRangeId,
    tracked_range: TrackedRange,
    update_policy: TrackedRangeUpdatePolicy,
}

impl FoldRange {
    pub fn new(id: FoldRangeId, version: BufferVersion, range: TextRange) -> Self {
        Self::with_policy(id, version, range, default_update_policy())
    }

    pub fn with_policy(
        id: FoldRangeId,
        version: BufferVersion,
        range: TextRange,
        update_policy: TrackedRangeUpdatePolicy,
    ) -> Self {
        Self {
            id,
            tracked_range: TrackedRange::from_range(version, range, fold_stickiness()),
            update_policy,
        }
    }

    pub fn id(&self) -> FoldRangeId {
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

    pub fn update_policy(&self) -> TrackedRangeUpdatePolicy {
        self.update_policy
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

/// 折叠区间默认 stickiness：两端插入时不主动扩张。
pub(super) const fn fold_stickiness() -> Stickiness {
    Stickiness::Never
}

/// 折叠区间默认 update policy：完全删除即失效，部分删除保留剩余。
pub(super) fn default_update_policy() -> TrackedRangeUpdatePolicy {
    TrackedRangeUpdatePolicy::invalidate_when_fully_deleted()
}
