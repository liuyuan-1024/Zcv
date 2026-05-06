//! M9B TrackedRange：由两个 Anchor 表达的可跟随文本变化区间。
//!
//! TrackedRange 只表达区间数学和版本推进策略，不承载 diagnostics、fold、
//! 搜索结果等业务含义。

use crate::{
    EngineResult,
    anchor::Anchor,
    errors::AnchorError,
    position_map::{Affinity, MappingResult, PositionMap, Stickiness},
    transaction::DeltaEvent,
    types::{BufferVersion, TextRange},
};

/// TrackedRange 遇到删除内容时是否失效。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TrackedRangeInvalidationPolicy {
    /// 永不因删除自动失效，只保留映射后的范围。
    #[default]
    Never,
    /// 原范围完全塌缩时失效。
    WhenFullyDeleted,
    /// 只要原范围被删除内容触碰就失效。
    WhenTouchedByDeletion,
}

/// TrackedRange 映射成空区间时的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TrackedRangeCollapsePolicy {
    /// 保留空区间。
    #[default]
    Keep,
    /// 空区间直接失效。
    Invalidate,
}

/// TrackedRange 通过一次变更时的策略组合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TrackedRangeUpdatePolicy {
    invalidation: TrackedRangeInvalidationPolicy,
    collapse: TrackedRangeCollapsePolicy,
}

impl TrackedRangeUpdatePolicy {
    pub fn new(
        invalidation: TrackedRangeInvalidationPolicy,
        collapse: TrackedRangeCollapsePolicy,
    ) -> Self {
        Self {
            invalidation,
            collapse,
        }
    }

    pub fn invalidate_when_fully_deleted() -> Self {
        Self::new(
            TrackedRangeInvalidationPolicy::WhenFullyDeleted,
            TrackedRangeCollapsePolicy::Keep,
        )
    }

    pub fn invalidate_when_touched_by_deletion() -> Self {
        Self::new(
            TrackedRangeInvalidationPolicy::WhenTouchedByDeletion,
            TrackedRangeCollapsePolicy::Keep,
        )
    }

    pub fn invalidate_when_collapsed() -> Self {
        Self::new(
            TrackedRangeInvalidationPolicy::Never,
            TrackedRangeCollapsePolicy::Invalidate,
        )
    }

    pub fn invalidation(self) -> TrackedRangeInvalidationPolicy {
        self.invalidation
    }

    pub fn collapse(self) -> TrackedRangeCollapsePolicy {
        self.collapse
    }
}

/// TrackedRange 映射后的高层结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackedRangeUpdate {
    /// 范围无删除触碰地映射到新版本。
    Mapped(TrackedRange),
    /// 范围被删除内容触碰，但仍保留映射后的范围。
    Deleted(TrackedRange),
    /// 非空范围塌缩为空范围，但仍保留。
    Collapsed(TrackedRange),
    /// 按策略失效；保留映射后的最后合法 range 供调用方做 UI 或日志处理。
    Invalidated {
        range: TextRange,
        version: BufferVersion,
    },
}

impl TrackedRangeUpdate {
    pub fn tracked_range(self) -> Option<TrackedRange> {
        match self {
            Self::Mapped(range) | Self::Deleted(range) | Self::Collapsed(range) => Some(range),
            Self::Invalidated { .. } => None,
        }
    }

    pub fn range(self) -> TextRange {
        match self {
            Self::Mapped(range) | Self::Deleted(range) | Self::Collapsed(range) => range.range(),
            Self::Invalidated { range, .. } => range,
        }
    }

    pub fn version(self) -> BufferVersion {
        match self {
            Self::Mapped(range) | Self::Deleted(range) | Self::Collapsed(range) => range.version(),
            Self::Invalidated { version, .. } => version,
        }
    }
}

/// 可跟随文本变化的区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackedRange {
    start: Anchor,
    end: Anchor,
    stickiness: Stickiness,
}

/// FoldedRange 当前复用 TrackedRange 的跟随数学；fold projection 自身属于后续阶段。
pub type FoldedRange = TrackedRange;

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
        Ok(self.map_through_position_map(event.new_version, &event.position_map))
    }

    pub fn map_through_delta_event_with_policy(
        self,
        event: &DeltaEvent,
        policy: TrackedRangeUpdatePolicy,
    ) -> Result<TrackedRangeUpdate, AnchorError> {
        self.verify_event_version(event)?;
        Ok(self.map_through_position_map_with_policy(
            event.new_version,
            &event.position_map,
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

    pub fn update_all_through_delta_event(
        ranges: &mut [Self],
        event: &DeltaEvent,
    ) -> Result<Vec<MappingResult<Self>>, AnchorError> {
        for range in ranges.iter().copied() {
            range.verify_event_version(event)?;
        }

        let mut updates = Vec::with_capacity(ranges.len());
        for range in ranges {
            let mapped = range.map_through_position_map(event.new_version, &event.position_map);
            *range = mapped.value();
            updates.push(mapped);
        }

        Ok(updates)
    }

    pub fn map_all_through_delta_event_with_policy(
        ranges: impl IntoIterator<Item = Self>,
        event: &DeltaEvent,
        policy: TrackedRangeUpdatePolicy,
    ) -> Result<Vec<TrackedRangeUpdate>, AnchorError> {
        ranges
            .into_iter()
            .map(|range| range.map_through_delta_event_with_policy(event, policy))
            .collect()
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
        if self.version() != event.old_version {
            return Err(AnchorError::VersionMismatch {
                expected: event.old_version,
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
    match result {
        MappingResult::Mapped(range) => {
            MappingResult::Mapped(TrackedRange::from_valid_range(version, range, stickiness))
        }
        MappingResult::Deleted(range) => {
            MappingResult::Deleted(TrackedRange::from_valid_range(version, range, stickiness))
        }
        MappingResult::Collapsed(range) => {
            MappingResult::Collapsed(TrackedRange::from_valid_range(version, range, stickiness))
        }
        MappingResult::Ambiguous(range) => {
            MappingResult::Ambiguous(TrackedRange::from_valid_range(version, range, stickiness))
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundarySide {
    Start,
    End,
}

fn boundary_affinity(stickiness: Stickiness, side: BoundarySide) -> Affinity {
    match stickiness {
        Stickiness::BeforeInsertion => Affinity::Before,
        Stickiness::AfterInsertion => Affinity::After,
        Stickiness::Expand => match side {
            BoundarySide::Start => Affinity::Before,
            BoundarySide::End => Affinity::After,
        },
        Stickiness::Never => match side {
            BoundarySide::Start => Affinity::After,
            BoundarySide::End => Affinity::Before,
        },
    }
}
