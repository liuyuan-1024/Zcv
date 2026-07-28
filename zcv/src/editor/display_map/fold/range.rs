//! 单条 Editor FoldRange：把折叠区间绑定到可跟随文本变化的 TrackedRange。
//!
//! Fold 只表达「这一段文本被折叠」事实；占位符样式、绘制和投影由 `projection` 模块承载。
//! 折叠区间使用 `Stickiness::Never`：用户显式折叠的范围不应该在两端插入时主动扩张。

use zcv_engine::{
    AnchorError, BufferVersion, DeltaEvent, Line, Stickiness, TextRange, TrackedRange,
    TrackedRangeUpdate, TrackedRangeUpdatePolicy,
};

use super::FoldRangeId;

/// 单条折叠区间与其可跟随文本的内部 TrackedRange。
///
/// `line_span` 是与 `tracked_range` 联动的缓存：fold 创建与 delta 应用后由 `FoldSet`
/// 统一刷新，使 `Projection::build` / 增量分类器在读取时不必再为每条 fold 做 byte→line
/// 的 O(log N) 转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FoldRange {
    id: FoldRangeId,
    tracked_range: TrackedRange,
    update_policy: TrackedRangeUpdatePolicy,
    line_span: (Line, Line),
}

impl FoldRange {
    pub(crate) fn with_policy(
        id: FoldRangeId,
        version: BufferVersion,
        range: TextRange,
        update_policy: TrackedRangeUpdatePolicy,
        line_span: (Line, Line),
    ) -> Self {
        Self {
            id,
            tracked_range: TrackedRange::from_range(version, range, fold_stickiness()),
            update_policy,
            line_span,
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

    /// 折叠区间覆盖的逻辑行闭区间 `[start_line, end_line]`，语义与 `fold_line_span` 一致。
    /// 由 `FoldSet` 在每次 fold 变更 / delta 应用后同步刷新。
    pub fn line_span(&self) -> (Line, Line) {
        self.line_span
    }

    pub fn start_line(&self) -> Line {
        self.line_span.0
    }

    pub fn end_line(&self) -> Line {
        self.line_span.1
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

    pub(super) fn set_line_span(&mut self, line_span: (Line, Line)) {
        self.line_span = line_span;
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
