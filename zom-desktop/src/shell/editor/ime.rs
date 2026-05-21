//! 编辑器 IME 能力：composition、UTF-16 / UTF-8 byte 坐标换算。

use std::ops::Range;

use zom_command::CommandError;
use zom_engine::{Buffer, ByteOffset, CompositionSelection, EngineError, SelectionSet};

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
        // 必须真正结束 composition，而不是留一个空 preedit 的壳：空壳会让
        // `marked_text_range` 仍报 `Some`，系统 IME 据此认为组合还在、把后续
        // 按键继续吞进它那个已经空了的会话 —— 表现为「取消候选后要多按一次
        // Esc 才退出新建」。
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
        let text = self.buffer.text();
        let start = utf16_to_byte_offset(text.as_ref(), range_utf16.start)
            .ok_or_else(|| CommandError::InvalidArgs("IME range start 越界".into()))?;
        let end = utf16_to_byte_offset(text.as_ref(), range_utf16.end)
            .ok_or_else(|| CommandError::InvalidArgs("IME range end 越界".into()))?;
        let selection = SelectionSet::new(vec![zom_engine::Selection::new(
            ByteOffset::new(start),
            ByteOffset::new(end),
        )]);
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
        let text = self.buffer.text();
        Some(
            byte_to_utf16_offset(text.as_ref(), range.start().get())
                ..byte_to_utf16_offset(text.as_ref(), range.end().get()),
        )
    }

    pub(crate) fn selected_range_utf16(&self) -> (Range<usize>, bool) {
        let primary = *self.selection.primary();
        let text = self.buffer.text();
        let range = byte_to_utf16_offset(text.as_ref(), primary.start().get())
            ..byte_to_utf16_offset(text.as_ref(), primary.end().get());
        (range, primary.is_reversed())
    }

    pub(crate) fn text_for_range_utf16(&self, range_utf16: Range<usize>) -> Option<String> {
        let text = self.buffer.text();
        let text_str = text.as_ref();
        let start_byte = utf16_to_byte_offset(text_str, range_utf16.start)?;
        let end_byte = utf16_to_byte_offset(text_str, range_utf16.end)?;
        if start_byte > end_byte {
            return None;
        }
        Some(text_str[start_byte..end_byte].to_string())
    }
}

fn map_engine_error(error: EngineError) -> CommandError {
    CommandError::ExecutionFailed(error.to_string())
}

/// 把 UTF-8 字节偏移换算成文档级 flat UTF-16 偏移。越界自动饱和到末尾。
fn byte_to_utf16_offset(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    let mut utf16 = 0usize;
    for (idx, ch) in text.char_indices() {
        if idx >= byte {
            return utf16;
        }
        utf16 += ch.len_utf16();
    }
    utf16
}

/// 把文档级 flat UTF-16 偏移换算回 UTF-8 字节偏移。
///
/// 落在 surrogate pair 中间视为越界返回 `None`，与 NSTextInputClient 的预期一致：
/// 调用方不应当在 surrogate pair 内部下手。
fn utf16_to_byte_offset(text: &str, target: usize) -> Option<usize> {
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
    let anchor = utf16_to_byte_offset(preedit, range_utf16.start)
        .ok_or_else(|| CommandError::InvalidArgs("IME preedit selection anchor 越界".into()))?;
    let head = utf16_to_byte_offset(preedit, range_utf16.end)
        .ok_or_else(|| CommandError::InvalidArgs("IME preedit selection head 越界".into()))?;
    Ok(CompositionSelection::new(
        ByteOffset::new(anchor),
        ByteOffset::new(head),
    ))
}
