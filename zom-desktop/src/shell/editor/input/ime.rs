//! 编辑器 IME 能力：composition、UTF-16 / UTF-8 byte 坐标换算。
//!
//! 系统输入法（macOS NSTextInputClient / Win TSF / Linux IBus）以"整文档 flat
//! UTF-16 offset"做选区，引擎内部用 `ByteOffset`。本模块只负责两套坐标系
//! **在文档边界上**的换算——所有 byte ↔ utf16-cu 走 engine 的 `byte_to_utf16_cu`
//! / `utf16_cu_to_byte`（O(log n)），**不再拷贝整 buffer 文本**，10G 文件 IME
//! 也不会卡顿。
//!
//! preedit（IME 候选高亮的小串）仍用本地 helper 线性扫——preedit 永远是
//! 几字到几十字，再大也只是一个候选词，不存在大文件问题。

use std::ops::Range;

use zom_command::CommandError;
use zom_engine::{
    Buffer, ByteOffset, CompositionSelection, EngineError, SelectionSet, Utf16Offset,
};

/// 可被系统输入法修改的编辑目标。
pub(crate) struct ImeTarget<'a> {
    buffer: &'a mut Buffer,
    selection: &'a mut SelectionSet,
}

/// 可被系统输入法查询的编辑目标。
pub(crate) struct ImeQueryTarget<'a> {
    buffer: &'a Buffer,
    selection: &'a SelectionSet,
}

impl<'a> ImeTarget<'a> {
    pub(crate) fn new(buffer: &'a mut Buffer, selection: &'a mut SelectionSet) -> Self {
        Self { buffer, selection }
    }

    /// 应用系统输入法给出的替换区间。区间使用 NSTextInputClient 语义下的
    /// UTF-16 offset，编辑器内部统一转换成 engine 的 byte selection。
    pub(crate) fn apply_replacement_range(
        &mut self,
        replacement_range_utf16: Option<Range<usize>>,
    ) -> Result<(), CommandError> {
        let Some(range_utf16) = replacement_range_utf16 else {
            return Ok(());
        };
        if self.buffer.is_composing() {
            return Ok(());
        }
        self.set_selection_from_utf16(range_utf16)
    }

    /// 更新输入法 preedit。commit 仍走命令通道，这里只处理 composition 的
    /// 即时展示与选区同步。
    pub(crate) fn replace_and_mark_text(
        &mut self,
        replacement_range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
    ) -> Result<(), CommandError> {
        self.apply_replacement_range(replacement_range_utf16)?;

        // 系统输入法把 marked text 置空 = 放弃组合（如按 Esc 取消候选）。
        // 必须真正结束 composition，而不是留一个空 preedit 的壳。
        // 空壳会让 `marked_text_range` 仍报 `Some`，系统 IME 据此认为组合还在、把后续按键继续吞进它那个已经空了的会话。
        // 表现为「取消候选后要多按一次 Esc 才退出新建」。
        if new_text.is_empty() {
            if self.buffer.is_composing() {
                self.buffer.cancel_composition().map_err(map_engine_error)?;
                *self.selection = self.buffer.selection().clone();
            }
            return Ok(());
        }

        let relative_selection = match new_selected_range_utf16 {
            Some(range) => Some(composition_selection_from_utf16(new_text, range)?),
            None => None,
        };
        self.buffer
            .update_composition(new_text, relative_selection)
            .map_err(map_engine_error)?;
        *self.selection = self.buffer.selection().clone();
        Ok(())
    }

    fn set_selection_from_utf16(&mut self, range_utf16: Range<usize>) -> Result<(), CommandError> {
        let start = self
            .buffer
            .utf16_cu_to_byte(Utf16Offset::new(range_utf16.start))
            .map_err(|_| CommandError::InvalidArgs("IME range start 越界".into()))?;
        let end = self
            .buffer
            .utf16_cu_to_byte(Utf16Offset::new(range_utf16.end))
            .map_err(|_| CommandError::InvalidArgs("IME range end 越界".into()))?;
        let selection = SelectionSet::new(vec![zom_engine::Selection::new(start, end)]);
        self.buffer
            .set_selection(selection.clone())
            .map_err(map_engine_error)?;
        *self.selection = selection;
        Ok(())
    }
}

impl<'a> ImeQueryTarget<'a> {
    pub(crate) fn new(buffer: &'a Buffer, selection: &'a SelectionSet) -> Self {
        Self { buffer, selection }
    }

    pub(crate) fn preedit_text(&self) -> Option<String> {
        self.buffer
            .composition()
            .map(|state| state.preedit_text().to_string())
    }

    pub(crate) fn marked_range_utf16(&self) -> Option<Range<usize>> {
        let range = self.buffer.composition()?.range();
        let start = self.buffer.byte_to_utf16_cu(range.start()).ok()?;
        let end = self.buffer.byte_to_utf16_cu(range.end()).ok()?;
        Some(start.get()..end.get())
    }

    pub(crate) fn selected_range_utf16(&self) -> (Range<usize>, bool) {
        let primary = *self.selection.primary();
        // 失败回退到 0..0 ——选区端点理应永远在合法字符边界，转换不应失败；
        // 真的失败时给 IME 一个最保守值，比 panic 强。
        let start = self
            .buffer
            .byte_to_utf16_cu(primary.start())
            .map(|v| v.get())
            .unwrap_or(0);
        let end = self
            .buffer
            .byte_to_utf16_cu(primary.end())
            .map(|v| v.get())
            .unwrap_or(start);
        (start..end, primary.is_reversed())
    }

    pub(crate) fn text_for_range_utf16(&self, range_utf16: Range<usize>) -> Option<String> {
        if range_utf16.start > range_utf16.end {
            return None;
        }
        let start_byte = self
            .buffer
            .utf16_cu_to_byte(Utf16Offset::new(range_utf16.start))
            .ok()?;
        let end_byte = self
            .buffer
            .utf16_cu_to_byte(Utf16Offset::new(range_utf16.end))
            .ok()?;
        let range = zom_engine::TextRange::new(start_byte, end_byte).ok()?;
        self.buffer
            .slice_text(range)
            .ok()
            .map(|slice| slice.as_str().to_string())
    }
}

fn map_engine_error(error: EngineError) -> CommandError {
    CommandError::ExecutionFailed(error.to_string())
}

/// 把本地小串的 flat UTF-16 偏移换算回 UTF-8 字节偏移。仅供 preedit
/// （IME 候选串）使用——preedit 永远小，线性扫描可接受；buffer 路径走
/// engine 的 `utf16_cu_to_byte`，不要拿这个去扫 buffer 全文。
///
/// 落在 surrogate pair 中间视为越界返回 `None`，与 NSTextInputClient 的预期一致：
/// 调用方不应当在 surrogate pair 内部下手。
fn utf16_to_byte_offset_in_str(text: &str, target: usize) -> Option<usize> {
    if target == 0 {
        return Some(0);
    }
    let mut utf16 = 0usize;
    for (idx, ch) in text.char_indices() {
        let step = ch.len_utf16();
        if utf16 + step > target {
            return None;
        }
        utf16 += step;
        if utf16 == target {
            return Some(idx + ch.len_utf8());
        }
    }
    if utf16 == target {
        Some(text.len())
    } else {
        None
    }
}

/// 把 preedit 内 UTF-16 区间换算成 byte 区间，构造 `CompositionSelection`。
fn composition_selection_from_utf16(
    preedit: &str,
    range_utf16: Range<usize>,
) -> Result<CompositionSelection, CommandError> {
    let anchor = utf16_to_byte_offset_in_str(preedit, range_utf16.start)
        .ok_or_else(|| CommandError::InvalidArgs("IME preedit selection anchor 越界".into()))?;
    let head = utf16_to_byte_offset_in_str(preedit, range_utf16.end)
        .ok_or_else(|| CommandError::InvalidArgs("IME preedit selection head 越界".into()))?;
    Ok(CompositionSelection::new(
        ByteOffset::new(anchor),
        ByteOffset::new(head),
    ))
}

#[cfg(test)]
mod tests {
    //! preedit 本地 helper 的健壮性测试。buffer 路径（byte_to_utf16_cu /
    //! utf16_cu_to_byte）由 engine 单元测试覆盖。

    use super::*;

    #[test]
    fn utf16_to_byte_offset_in_str_handles_surrogate_pair() {
        // "𐐷"在 BMP 外：4 UTF-8 字节，2 UTF-16 code unit。
        assert_eq!(utf16_to_byte_offset_in_str("𐐷", 0), Some(0));
        // 落在 surrogate pair 中间 → None。
        assert_eq!(utf16_to_byte_offset_in_str("𐐷", 1), None);
        assert_eq!(utf16_to_byte_offset_in_str("𐐷", 2), Some(4));
        // 越界。
        assert_eq!(utf16_to_byte_offset_in_str("𐐷", 3), None);
    }
}
