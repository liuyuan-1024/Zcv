//! Composition 状态数学：负责 preedit 相对选区、绝对选区和组合范围之间的纯坐标转换。
//!
//! 本文件不提交文本、不操作历史，也不决定 IME 生命周期；这些流程边界留给 `workflow`。

use crate::{ByteOffset, CompositionSelection, EngineResult, Selection, TextRange};

pub(in crate::buffer) fn resolve_relative_selection(
    selection: Option<CompositionSelection>,
    preedit_len: usize,
) -> CompositionSelection {
    selection.unwrap_or_else(|| CompositionSelection::caret(ByteOffset::new(preedit_len)))
}

pub(in crate::buffer) fn absolute_composition_selection(
    range_start: ByteOffset,
    selection: CompositionSelection,
) -> EngineResult<Selection> {
    Ok(Selection::new(
        ByteOffset::new(range_start.get() + selection.anchor().get()),
        ByteOffset::new(range_start.get() + selection.head().get()),
    ))
}

pub(in crate::buffer) fn composition_range_after_preedit(
    range_start: ByteOffset,
    preedit_len: usize,
) -> EngineResult<TextRange> {
    let range_end = ByteOffset::new(range_start.get() + preedit_len);
    Ok(TextRange::new(range_start, range_end)?)
}
