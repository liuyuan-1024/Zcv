//! MetadataRangeSpec：批量替换 layer 时使用的输入规格。
//!
//! 它让调用方一次性携带 range、stickiness、失效策略和 payload，避免公开 layer 内部构造顺序。

use crate::{position_map::Stickiness, tracking::TrackedRangeUpdatePolicy, types::TextRange};

/// 批量替换 metadata layer 时使用的输入项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataRangeSpec<T> {
    range: TextRange,
    stickiness: Stickiness,
    update_policy: TrackedRangeUpdatePolicy,
    metadata: T,
}

impl<T> MetadataRangeSpec<T> {
    pub fn new(range: TextRange, metadata: T) -> Self {
        Self {
            range,
            stickiness: Stickiness::default(),
            update_policy: TrackedRangeUpdatePolicy::default(),
            metadata,
        }
    }

    pub fn with_stickiness(mut self, stickiness: Stickiness) -> Self {
        self.stickiness = stickiness;
        self
    }

    pub fn with_update_policy(mut self, update_policy: TrackedRangeUpdatePolicy) -> Self {
        self.update_policy = update_policy;
        self
    }

    pub fn range(&self) -> TextRange {
        self.range
    }

    pub fn stickiness(&self) -> Stickiness {
        self.stickiness
    }

    pub fn update_policy(&self) -> TrackedRangeUpdatePolicy {
        self.update_policy
    }

    pub fn metadata(&self) -> &T {
        &self.metadata
    }

    pub(super) fn into_parts(self) -> (TextRange, Stickiness, TrackedRangeUpdatePolicy, T) {
        (
            self.range,
            self.stickiness,
            self.update_policy,
            self.metadata,
        )
    }
}
