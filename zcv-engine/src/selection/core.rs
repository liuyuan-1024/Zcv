//! Selection 类型：使用 anchor/head 模型表达单个 caret 或非空选区。
//!
//! 本文件只维护单个 selection 的方向、范围和映射；排序、合并和 primary 归属在 SelectionSet。

use crate::{ByteOffset, PositionMap, TextRange};

use super::Cursor;

/// 一个选区，使用 anchor/head 模型。
///
/// `anchor` 是固定端，`head` 是活动端。两者相等时表示 caret。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Selection {
    anchor: ByteOffset,
    head: ByteOffset,
}

impl Selection {
    pub const fn new(anchor: ByteOffset, head: ByteOffset) -> Self {
        Self { anchor, head }
    }

    pub const fn caret(offset: ByteOffset) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
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

    pub fn collapse_to_start(self) -> Self {
        Self::caret(self.start())
    }

    pub fn collapse_to_end(self) -> Self {
        Self::caret(self.end())
    }

    pub fn with_head(self, head: ByteOffset) -> Self {
        Self {
            anchor: self.anchor,
            head,
        }
    }

    pub fn map_through_position_map(self, position_map: &PositionMap) -> Self {
        Self {
            anchor: position_map.map_old_position(self.anchor).value(),
            head: position_map.map_old_position(self.head).value(),
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
        assert_eq!(reversed.collapse_to_start(), caret(2));
        assert_eq!(reversed.collapse_to_end(), caret(7));
    }
}
