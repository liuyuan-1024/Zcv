//! Selection 类型：使用 anchor/head 模型表达单个 caret 或非空选区。
//!
//! 本文件只维护单个 selection 的方向、范围和映射；排序、合并和 primary 归属在 SelectionSet。

use crate::{CharOffset, PositionMap, TextRange};

use super::Cursor;

/// 一个选区，使用 anchor/head 模型。
///
/// `anchor` 是固定端，`head` 是活动端。两者相等时表示 caret。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Selection {
    anchor: CharOffset,
    head: CharOffset,
}

impl Selection {
    pub const fn new(anchor: CharOffset, head: CharOffset) -> Self {
        Self { anchor, head }
    }

    pub const fn caret(offset: CharOffset) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    pub const fn anchor(self) -> CharOffset {
        self.anchor
    }

    pub const fn head(self) -> CharOffset {
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

    pub fn start(self) -> CharOffset {
        self.anchor.min(self.head)
    }

    pub fn end(self) -> CharOffset {
        self.anchor.max(self.head)
    }

    pub fn range(self) -> TextRange {
        TextRange::new(self.start(), self.end())
            .expect("Selection start/end 由 min/max 生成，必须满足 start <= end")
    }

    pub fn collapse_to_start(self) -> Self {
        Self::caret(self.start())
    }

    pub fn collapse_to_end(self) -> Self {
        Self::caret(self.end())
    }

    pub fn with_head(self, head: CharOffset) -> Self {
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
