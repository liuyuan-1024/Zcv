//! 跟随更新结果：把位置或区间映射后的事实暴露给调用方。
//!
//! Update 类型保留最后合法位置，方便宿主在失效时做 UI 清理或诊断记录。

use super::TrackedRange;
use crate::types::{BufferVersion, TextRange};

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
