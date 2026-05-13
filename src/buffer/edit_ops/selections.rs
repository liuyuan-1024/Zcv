//! Selection 编辑入口：把多光标、多选区插入、替换和删除归一化为单个事务。
//!
//! 本文件负责 selection 语义到 EditList 的映射，不实现底层提交原子性，也不绕过 Buffer 边界校验。

use std::sync::Arc;

use crate::{
    position_map::OffsetShift,
    storage::TextRead,
    transaction::{ChangeSet, Delta, Edit, Transaction, TransactionMetadata, TransactionSource},
    EngineError, EngineResult, MovementDirection, MovementUnit, Selection, SelectionSet, TextRange,
};

use crate::buffer::Buffer;

impl Buffer {
    /// 在每个 selection 处插入文本；非空 selection 会被替换。
    pub fn insert_at_selections(
        &mut self,
        selections: SelectionSet,
        text: &str,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.replace_selection_ranges_with_metadata(
            selections,
            text,
            TransactionMetadata::new(TransactionSource::Programmatic)
                .with_description("在选定位置插入"),
        )
    }

    /// 用同一段文本替换每个 selection。
    pub fn replace_selections(
        &mut self,
        selections: SelectionSet,
        replacement: &str,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.replace_selection_ranges_with_metadata(
            selections,
            replacement,
            TransactionMetadata::new(TransactionSource::Programmatic)
                .with_description("替换所选内容"),
        )
    }

    /// 删除所有非空 selection range；caret 本身不会删除任何字符。
    pub fn delete_selection_ranges(
        &mut self,
        selections: SelectionSet,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.replace_selection_ranges_with_metadata(
            selections,
            "",
            TransactionMetadata::new(TransactionSource::Programmatic)
                .with_description("删除所选内容"),
        )
    }

    /// 对每个 caret 执行 grapheme-safe Backspace；非空 selection 直接删除 selection range。
    pub fn delete_backward_at_selections(
        &mut self,
        selections: SelectionSet,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.delete_by_movement_at_selections(
            selections,
            MovementDirection::Previous,
            MovementUnit::Grapheme,
            TransactionMetadata::new(TransactionSource::Programmatic)
                .with_description("向后删除选定内容"),
        )
    }

    /// 对每个 caret 执行 grapheme-safe Delete；非空 selection 直接删除 selection range。
    pub fn delete_forward_at_selections(
        &mut self,
        selections: SelectionSet,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.delete_by_movement_at_selections(
            selections,
            MovementDirection::Next,
            MovementUnit::Grapheme,
            TransactionMetadata::new(TransactionSource::Programmatic)
                .with_description("向前删除所选内容"),
        )
    }

    pub(crate) fn delete_by_movement_at_selections(
        &mut self,
        selections: SelectionSet,
        direction: MovementDirection,
        unit: MovementUnit,
        metadata: TransactionMetadata,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.ensure_writable()?;
        self.validate_selection_set(&selections)?;

        let mut delete_targets = Vec::new();

        for selection in selections.as_slice() {
            if selection.is_caret() {
                let head = selection.head();
                // head 是 ByteOffset，movement_boundary 仍按 char 计算，跨边界转换。
                let head_char = self.storage.byte_to_char(head)?;
                let boundary_char = self.movement_boundary(head_char, direction, unit)?;
                let boundary = self.storage.char_to_byte(boundary_char)?;
                let range_selection = match direction {
                    MovementDirection::Previous => Selection::new(boundary, head),
                    MovementDirection::Next => Selection::new(head, boundary),
                };

                if !range_selection.range().is_empty() {
                    delete_targets.push(range_selection);
                }
            } else {
                delete_targets.push(*selection);
            }
        }

        if delete_targets.is_empty() {
            self.set_selection(selections)?;
            return Ok(None);
        }

        self.replace_selection_ranges_with_metadata(SelectionSet::new(delete_targets), "", metadata)
    }

    pub(crate) fn replace_selection_ranges_with_metadata(
        &mut self,
        selections: SelectionSet,
        replacement: &str,
        metadata: TransactionMetadata,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.ensure_writable()?;

        if metadata.source() != TransactionSource::Composition {
            self.cancel_composition_before_text_edit()?;
        }

        let selections = selections.normalized();
        self.validate_selection_set(&selections)?;

        let before_selection = selections.clone();
        // ByteOffset 深核：用 byte 长度
        let replacement_len = replacement.len();
        let replacement: Arc<str> = Arc::from(replacement);

        let mut edits = Vec::new();
        let mut after_selections = Vec::with_capacity(selections.len());
        let mut shift = OffsetShift::ZERO;

        for selection in selections.as_slice() {
            let range = selection.range();
            let new_start = shift
                .apply_old_to_new(range.start())
                .ok_or_else(|| offset_arithmetic_bug("replace_selection_ranges_with_metadata"))?;
            let new_head = new_start
                .checked_add(replacement_len)
                .ok_or_else(|| offset_arithmetic_bug("replace_selection_ranges_with_metadata"))?;

            let is_empty_noop = range.is_empty() && replacement.is_empty();
            // 流式比较，零拷贝
            let is_same_text_noop = if range.is_empty() {
                false
            } else if range.len() != replacement.len() {
                false
            } else {
                let mut consumed = 0usize;
                let mut equal = true;
                for chunk in self.storage.chunks(range)? {
                    let end = consumed + chunk.len();
                    if &replacement.as_bytes()[consumed..end] != chunk.as_bytes() {
                        equal = false;
                        break;
                    }
                    consumed = end;
                }
                equal && consumed == replacement.len()
            };

            if !is_empty_noop && !is_same_text_noop {
                edits.push(Edit::replace(range, Arc::clone(&replacement)));
                shift = shift
                    .after_edit(range.len(), replacement_len)
                    .ok_or_else(|| {
                        offset_arithmetic_bug("replace_selection_ranges_with_metadata")
                    })?;
            }

            after_selections.push(Selection::caret(new_head));
        }

        let after_selection = SelectionSet::new(after_selections);

        if edits.is_empty() {
            self.selection = after_selection;
            return Ok(None);
        }

        let tx = Transaction::from_edits(self.version, edits)?
            .with_metadata(metadata)
            .with_selection(Some(before_selection), Some(after_selection));

        self.apply_transaction(tx).map(Some)
    }

    pub(in crate::buffer) fn replace_single_range_with_metadata(
        &mut self,
        range: TextRange,
        replacement: &str,
        after_selection: SelectionSet,
        metadata: TransactionMetadata,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.ensure_writable()?;
        self.validate_range(range)?;
        self.validate_edit_boundary(range.start())?;
        self.validate_edit_boundary(range.end())?;

        if self.slice_text(range)?.as_ref() == replacement {
            self.selection = after_selection;
            return Ok(None);
        }

        let tx = Transaction::from_edits(
            self.version,
            vec![Edit::replace(range, Arc::<str>::from(replacement))],
        )?
        .with_metadata(metadata)
        .with_selection(Some(self.selection.clone()), Some(after_selection));

        self.apply_transaction(tx).map(Some)
    }
}

fn offset_arithmetic_bug(location: &'static str) -> EngineError {
    EngineError::EngineBug {
        location,
        detail: "byte offset overflow while mapping selection edits".to_string(),
    }
}
