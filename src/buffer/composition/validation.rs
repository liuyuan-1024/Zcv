//! Composition 校验：约束 preedit 内相对选区必须落在有效 char 范围和 grapheme 边界上。
//!
//! 本文件只表达组合态内部边界规则，不读取 Buffer 状态，也不负责提交、取消或恢复 selection。

use unicode_segmentation::UnicodeSegmentation;

use crate::{CharOffset, CompositionSelection, CoordinateError, EngineResult};

use crate::buffer::coordinates::char_to_byte_index;

pub(in crate::buffer) fn validate_composition_relative_selection(
    preedit_text: &str,
    selection: CompositionSelection,
) -> EngineResult<()> {
    let preedit_len = preedit_text.chars().count();

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

fn is_grapheme_boundary_in_str(text: &str, offset: CharOffset) -> EngineResult<bool> {
    let len_chars = text.chars().count();

    if offset.get() > len_chars {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    if offset.get() == 0 || offset.get() == len_chars {
        return Ok(true);
    }

    let byte_offset = char_to_byte_index(text, offset)?;
    Ok(text
        .grapheme_indices(true)
        .any(|(byte_index, _)| byte_index == byte_offset))
}
