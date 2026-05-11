//! Composition 校验：约束 preedit 内相对选区必须落在有效 byte 范围和 grapheme 边界上。
//!
//! 本文件只表达组合态内部边界规则，不读取 Buffer 状态，也不负责提交、取消或恢复 selection。

use unicode_segmentation::UnicodeSegmentation;

use crate::{ByteOffset, CompositionSelection, CoordinateError, EngineResult};

pub(in crate::buffer) fn validate_composition_relative_selection(
    preedit_text: &str,
    selection: CompositionSelection,
) -> EngineResult<()> {
    let preedit_len = preedit_text.len();

    for offset in [selection.anchor(), selection.head()] {
        if offset.get() > preedit_len {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        if !is_grapheme_boundary_in_str(preedit_text, offset)? {
            return Err(CoordinateError::InvalidGraphemeBoundary(offset).into());
        }
    }

    Ok(())
}

fn is_grapheme_boundary_in_str(text: &str, offset: ByteOffset) -> EngineResult<bool> {
    let len_bytes = text.len();

    if offset.get() > len_bytes {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    if offset.get() == 0 || offset.get() == len_bytes {
        return Ok(true);
    }

    // UTF-8 字符边界检查
    if !text.is_char_boundary(offset.get()) {
        return Ok(false);
    }

    Ok(text
        .grapheme_indices(true)
        .any(|(byte_index, _)| byte_index == offset.get()))
}
