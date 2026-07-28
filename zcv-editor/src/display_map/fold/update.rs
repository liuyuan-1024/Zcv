//! Editor FoldRangeUpdate：把 TrackedRangeUpdate 转换成带 FoldRangeId 的折叠更新事实。
//!
//! 这层结果服务宿主清理、UI 同步与日志，不改变 fold 集合的归属语义。

use zcv_engine::{BufferVersion, TextRange, TrackedRangeUpdate};

use super::FoldRangeId;

/// 单条 FoldRange 通过一个组合 Patch 后的更新事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FoldRangeUpdate {
    /// 区间无删除触碰地推进到新版本，fold 可以原样保留。
    Mapped {
        id: FoldRangeId,
        range: TextRange,
        version: BufferVersion,
    },
    /// 原区间被删除内容触碰，但按策略仍保留折算后的区间。
    Deleted {
        id: FoldRangeId,
        range: TextRange,
        version: BufferVersion,
    },
    /// 原非空区间映射后成为空区间，并且当前策略允许继续保留。
    Collapsed {
        id: FoldRangeId,
        range: TextRange,
        version: BufferVersion,
    },
    /// 当前 update policy 判定 range 不应继续存在，返回最后合法位置供宿主清理。
    Invalidated {
        id: FoldRangeId,
        range: TextRange,
        version: BufferVersion,
    },
}

impl FoldRangeUpdate {
    pub fn id(self) -> FoldRangeId {
        match self {
            Self::Mapped { id, .. }
            | Self::Deleted { id, .. }
            | Self::Collapsed { id, .. }
            | Self::Invalidated { id, .. } => id,
        }
    }

    pub fn range(self) -> TextRange {
        match self {
            Self::Mapped { range, .. }
            | Self::Deleted { range, .. }
            | Self::Collapsed { range, .. }
            | Self::Invalidated { range, .. } => range,
        }
    }

    pub fn version(self) -> BufferVersion {
        match self {
            Self::Mapped { version, .. }
            | Self::Deleted { version, .. }
            | Self::Collapsed { version, .. }
            | Self::Invalidated { version, .. } => version,
        }
    }

    pub fn is_invalidated(self) -> bool {
        matches!(self, Self::Invalidated { .. })
    }

    pub(super) fn from_tracked(id: FoldRangeId, update: TrackedRangeUpdate) -> Self {
        match update {
            TrackedRangeUpdate::Mapped(range) => Self::Mapped {
                id,
                range: range.range(),
                version: range.version(),
            },
            TrackedRangeUpdate::Deleted(range) => Self::Deleted {
                id,
                range: range.range(),
                version: range.version(),
            },
            TrackedRangeUpdate::Collapsed(range) => Self::Collapsed {
                id,
                range: range.range(),
                version: range.version(),
            },
            TrackedRangeUpdate::Invalidated { range, version } => {
                Self::Invalidated { id, range, version }
            }
        }
    }
}
