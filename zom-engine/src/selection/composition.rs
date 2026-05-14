//! Selection 侧的 composition 数据类型：表达 IME preedit 内部相对选区和 Buffer 中的组合态。
//!
//! 组合输入的提交、取消和校验流程在 `buffer/composition/`，这里不直接修改文本。

use crate::{ByteOffset, TextRange};

use super::{Selection, SelectionSet};

/// 组合输入中的相对选区。
///
/// `anchor` / `head` 是相对于当前 preedit 文本开头的 `ByteOffset`，
/// 不是整个 Buffer 的绝对坐标。`Buffer::update_composition` 会把它映射为
/// 当前文档中的绝对 `Selection`，并验证它没有落在 grapheme 中间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CompositionSelection {
    anchor: ByteOffset,
    head: ByteOffset,
}

impl CompositionSelection {
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
}

/// 当前组合输入状态。
///
/// `range` 是当前 preedit 文本在 Buffer 中的绝对范围；`selection` 是当前组合态
/// 选区在 Buffer 中的绝对位置。`original_*` 字段用于 cancel / commit 时恢复
/// 或生成合理的单步 Undo 历史。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionState {
    pub(crate) original_text: String,
    pub(crate) original_selection: SelectionSet,
    pub(crate) original_was_dirty: bool,
    pub(crate) range: TextRange,
    pub(crate) preedit_text: String,
    pub(crate) selection: Selection,
}

impl CompositionState {
    pub(crate) fn new(
        original_text: String,
        original_selection: SelectionSet,
        original_was_dirty: bool,
        range: TextRange,
    ) -> Self {
        Self {
            original_text,
            original_selection,
            original_was_dirty,
            range,
            preedit_text: String::new(),
            selection: Selection::caret(range.start()),
        }
    }

    pub fn range(&self) -> TextRange {
        self.range
    }

    pub fn preedit_text(&self) -> &str {
        &self.preedit_text
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    pub fn original_selection(&self) -> &SelectionSet {
        &self.original_selection
    }

    pub fn original_was_dirty(&self) -> bool {
        self.original_was_dirty
    }
}
