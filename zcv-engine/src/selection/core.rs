//! Selection 类型：使用 anchor/head 模型表达单个 caret 或非空选区。
//!
//! 本文件只维护单个 selection 的方向、范围和映射；排序、合并和 primary 归属在 SelectionSet。

use super::Cursor;
use crate::{ByteOffset, PositionMap, TextRange};

/// 一个选区，使用 anchor/head 模型。
///
/// 固定端与活动端；两者相等时表示 caret。
/// 垂直移动时持久保留的目标显示列：目标行比目标列短时光标被钳制到行尾，但目标列保留，下一次垂直移动仍回到原目标列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Selection {
    anchor: ByteOffset,
    head: ByteOffset,
    goal: Option<usize>,
}

impl Selection {
    pub const fn new(anchor: ByteOffset, head: ByteOffset) -> Self {
        Self {
            anchor,
            head,
            goal: None,
        }
    }

    pub const fn caret(offset: ByteOffset) -> Self {
        Self {
            anchor: offset,
            head: offset,
            goal: None,
        }
    }

    /// 设置垂直移动持久保留的目标显示列数值；`None` 表示从当前位置推导。
    pub const fn with_goal(mut self, goal: Option<usize>) -> Self {
        self.goal = goal;
        self
    }

    /// 垂直移动持久保留的目标显示列数值；`None` 表示未设置（对齐 Zed `SelectionGoal::None`）。
    pub const fn goal(self) -> Option<usize> {
        self.goal
    }

    pub const fn anchor(self) -> ByteOffset {
        self.anchor
    }

    pub const fn head(self) -> ByteOffset {
        self.head
    }

    pub fn cursor(self) -> Option<Cursor> {
        if self.is_caret() {
            Some(Cursor::new(self.head))
        } else {
            None
        }
    }

    pub fn is_caret(self) -> bool {
        self.anchor == self.head
    }

    pub fn is_reversed(self) -> bool {
        self.anchor > self.head
    }

    pub fn start(self) -> ByteOffset {
        self.anchor.min(self.head)
    }

    pub fn end(self) -> ByteOffset {
        self.anchor.max(self.head)
    }

    pub fn range(self) -> TextRange {
        TextRange::new(self.start(), self.end())
            .expect("Selection 的 start 和 end 由 min/max 生成，必须满足 start <= end")
    }

    /// 移动 head 到新位置；垂直扩展选区时保留 goal。
    pub fn with_head(self, head: ByteOffset) -> Self {
        Self {
            anchor: self.anchor,
            head,
            goal: self.goal,
        }
    }

    /// 编辑后按 PositionMap 映射坐标；goal 随选区保留，等待下一次移动决策。
    pub fn map_through_position_map(self, position_map: &PositionMap) -> Self {
        Self {
            anchor: position_map.map_old_position(self.anchor).value(),
            head: position_map.map_old_position(self.head).value(),
            goal: self.goal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(value: usize) -> ByteOffset {
        ByteOffset::new(value)
    }

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(b(start), b(end)).unwrap()
    }

    fn selection(anchor: usize, head: usize) -> Selection {
        Selection::new(b(anchor), b(head))
    }

    fn caret(offset: usize) -> Selection {
        Selection::caret(b(offset))
    }

    #[test]
    fn selection_and_cursor_contract_should_preserve_anchor_head_direction_and_range() {
        let cursor = Cursor::new(b(3));
        let reversed = selection(7, 2);

        assert_eq!(cursor.offset(), b(3));
        assert_eq!(cursor.to_selection(), caret(3));
        assert_eq!(reversed.anchor(), b(7));
        assert_eq!(reversed.head(), b(2));
        assert!(reversed.is_reversed());
        assert_eq!(reversed.range(), range(2, 7));
    }
}
