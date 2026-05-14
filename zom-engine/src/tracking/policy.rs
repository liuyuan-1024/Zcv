//! 跟随策略：定义 Anchor 和 TrackedRange 遇到删除/塌缩时是否继续保留。
//!
//! 策略是纯数据；实际映射由 Anchor、TrackedRange 和 PositionMap 协作完成。

/// Anchor 落在被删除旧内容中时的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AnchorDeletedPolicy {
    /// 保留 Anchor，并把它折叠到删除后的最近合法位置。
    #[default]
    Collapse,
    /// 不再保留 Anchor，只返回折叠后的轻量 Mark 供调用方决定后续处理。
    Invalidate,
}

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
