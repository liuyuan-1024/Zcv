//! Anchor：绑定 `BufferVersion` 的稳定位置标记。
//!
//! 所有文本更新都沿同一版本化坐标链推进；
//! `resolve_in` 通过 Buffer 的不衰减坐标索引重新定位，不依赖会被预算裁剪的带文本编辑日志。

use std::ops::Range;

use super::insertion_index::InsertionId;
use crate::{
    position_map::Affinity,
    types::{BufferVersion, ByteOffset, TextRange},
};

/// 绑定 BufferVersion 的稳定位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Anchor {
    version: BufferVersion,
    offset: ByteOffset,
    affinity: Affinity,
    /// 稳定插入身份；空文档或未绑定时为 NONE。
    insertion: InsertionId,
    /// 插入内偏移；同一插入内用它区分顺序。
    insertion_offset: u32,
}

impl Anchor {
    pub fn new(version: BufferVersion, offset: ByteOffset) -> Self {
        Self {
            version,
            offset,
            affinity: Affinity::default(),
            insertion: InsertionId::NONE,
            insertion_offset: 0,
        }
    }

    /// 绑定稳定插入身份；由快照在创建锚点时填入。
    pub(crate) fn with_insertion(mut self, insertion: InsertionId, insertion_offset: u32) -> Self {
        self.insertion = insertion;
        self.insertion_offset = insertion_offset;
        self
    }

    pub(crate) fn insertion(self) -> InsertionId {
        self.insertion
    }

    pub(crate) fn insertion_offset(self) -> u32 {
        self.insertion_offset
    }

    pub fn with_affinity(mut self, affinity: Affinity) -> Self {
        self.affinity = affinity;
        self
    }

    pub fn version(self) -> BufferVersion {
        self.version
    }

    pub fn offset(self) -> ByteOffset {
        self.offset
    }

    pub fn affinity(self) -> Affinity {
        self.affinity
    }

    /// 创建一个不吸收边界插入的锚点范围。
    ///
    /// 起点贴在插入内容之后，终点贴在插入内容之前；
    /// 适合折叠等只跟随原有文本而不扩张的范围。
    pub fn range_inside(version: BufferVersion, range: TextRange) -> Range<Self> {
        Self::new(version, range.start()).with_affinity(Affinity::After)
            ..Self::new(version, range.end()).with_affinity(Affinity::Before)
    }

    /// 创建一个吸收边界插入的锚点范围。
    ///
    /// 起点贴在插入内容之前，终点贴在插入内容之后。
    /// 适合自动闭合对这类需要把内部新输入继续纳入范围的标记。
    pub fn range_outside(version: BufferVersion, range: TextRange) -> Range<Self> {
        Self::new(version, range.start()).with_affinity(Affinity::Before)
            ..Self::new(version, range.end()).with_affinity(Affinity::After)
    }

    /// 把锚点解析到目标快照的当前坐标。
    ///
    /// 目标快照更旧返回 [`AnchorError::TargetBeforeSource`]。
    /// 调用方必须显式处理失败，不得把锚点原始偏移当成目标快照坐标。
    pub fn resolve_in(&self, snapshot: &crate::Snapshot) -> crate::TextResult<ByteOffset> {
        let map = snapshot.position_map_since(self.version)?;
        Ok(map
            .map_old_position_with_affinity(self.offset, self.affinity)
            .value())
    }
}

impl Default for Anchor {
    fn default() -> Self {
        Self::new(BufferVersion::INITIAL, ByteOffset::ZERO)
    }
}

#[cfg(test)]
#[path = "test/anchor_tests.rs"]
mod tests;
