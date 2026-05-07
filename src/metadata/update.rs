//! MetadataRangeUpdate：把 TrackedRangeUpdate 转换成带 MetadataRangeId 的 layer 更新事实。
//!
//! 这层结果服务宿主清理、日志和 UI 同步，不改变 metadata payload 本身。

use crate::{
    tracking::TrackedRangeUpdate,
    types::{BufferVersion, TextRange},
};

use super::MetadataRangeId;

/// 单条 metadata range 通过一次 DeltaEvent 后的更新事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetadataRangeUpdate {
    /// 区间无删除触碰地推进到新版本，metadata range 可以原样保留。
    Mapped {
        id: MetadataRangeId,
        range: TextRange,
        version: BufferVersion,
    },
    /// 原区间被删除内容触碰，但按策略仍保留折算后的区间。
    Deleted {
        id: MetadataRangeId,
        range: TextRange,
        version: BufferVersion,
    },
    /// 原非空区间映射后成为空区间，并且当前策略允许继续保留。
    Collapsed {
        id: MetadataRangeId,
        range: TextRange,
        version: BufferVersion,
    },
    /// 当前 update policy 判定 range 不应继续存在，返回最后合法位置供宿主清理或展示。
    Invalidated {
        id: MetadataRangeId,
        range: TextRange,
        version: BufferVersion,
    },
}

impl MetadataRangeUpdate {
    pub fn id(self) -> MetadataRangeId {
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

    pub(super) fn from_tracked(id: MetadataRangeId, update: TrackedRangeUpdate) -> Self {
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
