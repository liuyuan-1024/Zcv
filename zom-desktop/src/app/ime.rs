//! IME 桥接与 UTF-16 / UTF-8 byte 坐标换算。

use std::ops::Range;

use crate::app::App;

use zom_command::CommandError;
use zom_command::commands::editor;
use zom_engine::{ByteOffset, CompositionSelection, EngineError, SelectionSet};

impl App {
    /// 提交系统输入法文本。commit 走命令路径，保证进入 undo 历史。
    pub(crate) fn ime_replace_text(
        &mut self,
        replacement_range_utf16: Option<Range<usize>>,
        text: &str,
    ) -> Result<(), CommandError> {
        if let Some(range_utf16) = replacement_range_utf16 {
            if !self.with_active_buffer(|buffer| buffer.is_composing())? {
                self.set_selection_from_utf16(range_utf16)?;
            }
        }

        self.dispatch(editor::ime_commit(text))?;
        Ok(())
    }

    /// 更新输入法 preedit。update 走直接通道，避免每次按键都过命令队列。
    pub(crate) fn ime_replace_and_mark_text(
        &mut self,
        replacement_range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
    ) -> Result<(), CommandError> {
        if let Some(range_utf16) = replacement_range_utf16 {
            if !self.with_active_buffer(|buffer| buffer.is_composing())? {
                self.set_selection_from_utf16(range_utf16)?;
            }
        }

        let relative_selection = match new_selected_range_utf16 {
            Some(range) => Some(composition_selection_from_utf16(new_text, range)?),
            None => None,
        };

        let after_selection = self.with_active_buffer_mut(|buffer| {
            buffer
                .update_composition(new_text, relative_selection)
                .map_err(map_engine_error)?;
            Ok(buffer.selection().clone())
        })?;
        self.sync_active_view_selection(after_selection)?;
        Ok(())
    }

    pub(crate) fn ime_unmark(&mut self) -> Result<(), CommandError> {
        let preedit = self.with_active_buffer(|buffer| {
            buffer
                .composition()
                .map(|state| state.preedit_text().to_string())
        })?;
        let Some(preedit) = preedit else {
            return Ok(());
        };
        self.dispatch(editor::ime_commit(preedit))?;
        Ok(())
    }

    pub(crate) fn ime_marked_range_utf16(&self) -> Option<Range<usize>> {
        let view = self.views.active_view()?;
        let buffer = self.workspace.buffer(view.buffer())?.buffer();
        let range = buffer.composition()?.range();
        let text = buffer.text();
        Some(
            byte_to_utf16_offset(text.as_ref(), range.start().get())
                ..byte_to_utf16_offset(text.as_ref(), range.end().get()),
        )
    }

    pub(crate) fn ime_selected_range_utf16(&self) -> Option<(Range<usize>, bool)> {
        let view = self.views.active_view()?;
        let buffer = self.workspace.buffer(view.buffer())?.buffer();
        let primary = *view.selection().primary();
        let text = buffer.text();
        let range = byte_to_utf16_offset(text.as_ref(), primary.start().get())
            ..byte_to_utf16_offset(text.as_ref(), primary.end().get());
        Some((range, primary.is_reversed()))
    }

    pub(crate) fn ime_text_for_range_utf16(&self, range_utf16: Range<usize>) -> Option<String> {
        let view = self.views.active_view()?;
        let buffer = self.workspace.buffer(view.buffer())?.buffer();
        let text = buffer.text();
        let text_str = text.as_ref();
        let start_byte = utf16_to_byte_offset(text_str, range_utf16.start)?;
        let end_byte = utf16_to_byte_offset(text_str, range_utf16.end)?;
        if start_byte > end_byte {
            return None;
        }
        Some(text_str[start_byte..end_byte].to_string())
    }

    fn with_active_buffer<R>(
        &self,
        f: impl FnOnce(&zom_engine::Buffer) -> R,
    ) -> Result<R, CommandError> {
        let view = self.views.active_view().ok_or(CommandError::NoActiveView)?;
        let buffer = self
            .workspace
            .buffer(view.buffer())
            .ok_or(CommandError::BufferNotFound(view.buffer()))?
            .buffer();
        Ok(f(buffer))
    }

    fn with_active_buffer_mut<R>(
        &mut self,
        f: impl FnOnce(&mut zom_engine::Buffer) -> Result<R, CommandError>,
    ) -> Result<R, CommandError> {
        let buffer_id = self
            .views
            .active_view()
            .map(|view| view.buffer())
            .ok_or(CommandError::NoActiveView)?;
        let buffer = self
            .workspace
            .buffer_mut(buffer_id)
            .ok_or(CommandError::BufferNotFound(buffer_id))?
            .buffer_mut();
        f(buffer)
    }

    fn sync_active_view_selection(&mut self, selection: SelectionSet) -> Result<(), CommandError> {
        let view = self
            .views
            .active_view_mut()
            .ok_or(CommandError::NoActiveView)?;
        *view.selection_mut() = selection;
        Ok(())
    }

    /// 把一段 UTF-16 区间映射成 byte 区间，再压成 primary selection。
    /// 用于 IME `replacementRange` 在没有 active composition 时定位插入点。
    fn set_selection_from_utf16(&mut self, range_utf16: Range<usize>) -> Result<(), CommandError> {
        let byte_range = self.with_active_buffer(|buffer| {
            let text = buffer.text();
            let start = utf16_to_byte_offset(text.as_ref(), range_utf16.start)
                .ok_or_else(|| CommandError::InvalidArgs("IME range start 越界".into()))?;
            let end = utf16_to_byte_offset(text.as_ref(), range_utf16.end)
                .ok_or_else(|| CommandError::InvalidArgs("IME range end 越界".into()))?;
            Ok::<Range<usize>, CommandError>(start..end)
        })??;
        let selection = SelectionSet::new(vec![zom_engine::Selection::new(
            ByteOffset::new(byte_range.start),
            ByteOffset::new(byte_range.end),
        )]);
        self.with_active_buffer_mut(|buffer| {
            buffer
                .set_selection(selection.clone())
                .map_err(map_engine_error)
        })?;
        self.sync_active_view_selection(selection)
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
