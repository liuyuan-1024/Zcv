//! Selection 类型：泛型选区，偏移态与源锚点态共用同一套语义。
//!
//! 单个选区只维护有序端点、方向与垂直移动目标；排序、合并和 primary 归属在 SelectionSet。
//! 对齐 Zed 的 text::Selection<T>：start/end 始终有序，方向由 reversed 表达。

use zcv_multi_buffer::{
    MultiBufferAnchor, MultiBufferOffset, MultiBufferRange, MultiBufferSnapshot,
};
use zcv_text::Affinity;

/// 一个选区，使用有序端点 + 方向模型。
///
/// start <= end 始终成立；reversed 表示活动端在 start 侧。
/// 两端相等时表示 caret。
/// 垂直移动时持久保留的目标显示列：目标行比目标列短时光标被钳制到行尾，但目标列保留，下一次垂直移动仍回到原目标列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Selection<T = MultiBufferOffset> {
    start: T,
    end: T,
    reversed: bool,
    goal: Option<usize>,
}

impl<T: Copy> Selection<T> {
    /// 由固定端（tail）与活动端（head）构造，方向由两者次序决定。
    pub fn new(tail: T, head: T) -> Self
    where
        T: Ord,
    {
        if head < tail {
            Self::from_parts(head, tail, true, None)
        } else {
            Self::from_parts(tail, head, false, None)
        }
    }

    /// 零宽 caret。
    pub const fn caret(offset: T) -> Self {
        Self::from_parts(offset, offset, false, None)
    }

    /// 由已排序端点与方向直接构造；跨坐标空间转换时保留方向语义。
    pub(crate) const fn from_parts(start: T, end: T, reversed: bool, goal: Option<usize>) -> Self {
        Self {
            start,
            end,
            reversed,
            goal,
        }
    }

    pub const fn start(self) -> T {
        self.start
    }

    pub const fn end(self) -> T {
        self.end
    }

    pub const fn reversed(self) -> bool {
        self.reversed
    }

    /// 活动端：reversed 时为 start，否则为 end。
    pub fn head(self) -> T {
        if self.reversed { self.start } else { self.end }
    }

    /// 固定端：reversed 时为 end，否则为 start。
    pub fn tail(self) -> T {
        if self.reversed { self.end } else { self.start }
    }

    pub fn is_caret(self) -> bool
    where
        T: PartialEq,
    {
        self.start == self.end
    }

    /// 设置垂直移动持久保留的目标显示列数值；None 表示从当前位置推导。
    pub const fn with_goal(mut self, goal: Option<usize>) -> Self {
        self.goal = goal;
        self
    }

    /// 垂直移动持久保留的目标显示列数值；None 表示未设置。
    pub const fn goal(self) -> Option<usize> {
        self.goal
    }
}

impl<T: Copy + Ord> Selection<T> {
    /// 移动活动端到新位置，固定端不变，方向随之更新。
    pub fn with_head(self, head: T) -> Self {
        let tail = self.tail();
        Self::new(tail, head).with_goal(self.goal)
    }
}

impl Selection<MultiBufferOffset> {
    pub fn range(self) -> MultiBufferRange {
        MultiBufferRange::new(self.start, self.end)
            .expect("Selection 的 start 和 end 由有序端点构造，必须满足 start <= end")
    }

    /// 把偏移选区锚定为源锚点选区。
    ///
    /// caret 两端吸附插入文本之后；非空选区左端吸附插入之前、右端之后，边界插入不撑大选区。
    pub(crate) fn anchored(self, snapshot: &MultiBufferSnapshot) -> Selection<MultiBufferAnchor> {
        let (start_affinity, end_affinity) = if self.is_caret() {
            (Affinity::After, Affinity::After)
        } else {
            (Affinity::Before, Affinity::After)
        };
        Selection::from_parts(
            snapshot.anchor_at(self.start, start_affinity),
            snapshot.anchor_at(self.end, end_affinity),
            self.reversed,
            self.goal,
        )
    }
}

impl Selection<MultiBufferAnchor> {
    /// 按当前快照解析为偏移选区。
    pub(crate) fn resolve(
        self,
        snapshot: &MultiBufferSnapshot,
    ) -> Option<Selection<MultiBufferOffset>> {
        Some(Selection::from_parts(
            snapshot.resolve_anchor(&self.start)?,
            snapshot.resolve_anchor(&self.end)?,
            self.reversed,
            self.goal,
        ))
    }
}

#[cfg(test)]
#[path = "test/core_tests.rs"]
mod tests;
