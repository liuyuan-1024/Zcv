//! Cursor 类型：Selection 的零宽特例，用于表达单个插入点。
//!
//! Cursor 不单独承担多光标归一化；集合语义统一由 SelectionSet 处理。

use super::Selection;
use crate::ByteOffset;

/// 单个插入光标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Cursor {
    offset: ByteOffset,
}

impl Cursor {
    pub const fn new(offset: ByteOffset) -> Self {
        Self { offset }
    }

    pub const fn offset(self) -> ByteOffset {
        self.offset
    }

    pub const fn to_selection(self) -> Selection {
        Selection::caret(self.offset)
    }
}
