use crate::{
    BufferVersion, CharOffset, EngineError, EngineResult, Selection, SelectionSet, TextRange,
    storage::TextStorage,
    transaction::{
        ChangeSet, Delta, Edit, EditList, Transaction, TransactionMetadata, TransactionSource,
    },
};

use super::{Buffer, history::HistoryEntry};

impl Buffer {
    /// 提交并应用事务。
    ///
    /// 成功将返回增量事件 Delta 和位置映射器 ChangeSet，并记录 Undo 历史。
    pub fn apply_transaction(&mut self, tx: Transaction) -> EngineResult<(Delta, ChangeSet)> {
        if tx.metadata().source != TransactionSource::Composition {
            self.cancel_composition_before_text_edit()?;
        }

        let (base_version, tx_edits, metadata, tx_before_selection, tx_after_selection) =
            tx.into_parts();

        if base_version != self.version {
            return Err(crate::TransactionError::VersionMismatch {
                expected: self.version,
                actual: base_version,
            }
            .into());
        }

        self.validate_edit_list(&tx_edits)?;

        let before_text = self.text().into_owned();
        let before_selection = tx_before_selection.unwrap_or_else(|| self.selection.clone());
        let undo_edits = Self::build_inverse_edit_list(&before_text, &tx_edits)?;
        let redo_edits = tx_edits.clone();

        let (delta, changeset) = self.apply_edit_list(base_version, tx_edits)?;

        let after_selection = tx_after_selection
            .unwrap_or_else(|| before_selection.map_through_changeset(&changeset));
        self.selection = after_selection.clone();

        let after_text = self.text().into_owned();

        if metadata.record_history {
            let entry = HistoryEntry::new(
                before_text,
                after_text,
                undo_edits,
                redo_edits,
                before_selection,
                after_selection,
                metadata.description.clone(),
            );

            self.push_history(entry, &metadata)?;
        } else {
            // 任何新的文本变异都会让已有 redo 分支失效；Undo / Redo 自身走
            // apply_edit_list，不会触发这里。
            self.redo_stack.clear();
        }

        Ok((delta, changeset))
    }

    pub fn insert(&mut self, offset: CharOffset, text: &str) -> EngineResult<()> {
        let range = TextRange::new(offset, offset)?;
        self.replace(range, text)
    }

    pub fn delete(&mut self, range: TextRange) -> EngineResult<()> {
        self.replace(range, "")
    }

    /// 替换指定字符范围的文本，支持插入和删除。
    ///
    /// M3 起该便利 API 也会走 Transaction，从而进入 Undo 历史。
    pub fn replace(&mut self, range: TextRange, replacement: &str) -> EngineResult<()> {
        self.cancel_composition_before_text_edit()?;
        self.validate_range(range)?;
        self.validate_edit_boundary(range.start())?;
        self.validate_edit_boundary(range.end())?;

        // no-op 不递增版本，也不污染 dirty / history。
        if self.slice_text(range)?.as_ref() == replacement {
            return Ok(());
        }

        let tx = Transaction::from_edits(
            self.version,
            vec![Edit::replace(range, replacement.to_string())],
        )?;

        self.apply_transaction(tx)?;
        Ok(())
    }

    /// 在每个 selection 处插入文本；非空 selection 会被替换。
    pub fn insert_at_selections(
        &mut self,
        selections: SelectionSet,
        text: &str,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.replace_selection_ranges_with_metadata(
            selections,
            text,
            TransactionMetadata::new(TransactionSource::Keyboard)
                .with_description("insert at selections"),
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
            TransactionMetadata::new(TransactionSource::Command)
                .with_description("replace selections"),
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
            TransactionMetadata::new(TransactionSource::Delete)
                .with_description("delete selections"),
        )
    }

    /// 对每个 caret 执行 grapheme-safe Backspace；非空 selection 直接删除 selection range。
    pub fn delete_backward_at_selections(
        &mut self,
        selections: SelectionSet,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.validate_selection_set(&selections)?;

        let mut delete_targets = Vec::new();

        for selection in selections.as_slice() {
            if selection.is_caret() {
                let end = selection.head();
                let start = self.previous_grapheme_boundary(end)?;

                if start != end {
                    delete_targets.push(Selection::new(start, end));
                }
            } else {
                delete_targets.push(*selection);
            }
        }

        if delete_targets.is_empty() {
            self.set_selection(selections)?;
            return Ok(None);
        }

        self.replace_selection_ranges_with_metadata(
            SelectionSet::new(delete_targets),
            "",
            TransactionMetadata::new(TransactionSource::Delete)
                .with_description("delete backward at selections"),
        )
    }

    /// 对每个 caret 执行 grapheme-safe Delete；非空 selection 直接删除 selection range。
    pub fn delete_forward_at_selections(
        &mut self,
        selections: SelectionSet,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.validate_selection_set(&selections)?;

        let mut delete_targets = Vec::new();

        for selection in selections.as_slice() {
            if selection.is_caret() {
                let start = selection.head();
                let end = self.next_grapheme_boundary(start)?;

                if start != end {
                    delete_targets.push(Selection::new(start, end));
                }
            } else {
                delete_targets.push(*selection);
            }
        }

        if delete_targets.is_empty() {
            self.set_selection(selections)?;
            return Ok(None);
        }

        self.replace_selection_ranges_with_metadata(
            SelectionSet::new(delete_targets),
            "",
            TransactionMetadata::new(TransactionSource::Delete)
                .with_description("delete forward at selections"),
        )
    }

    pub(super) fn replace_selection_ranges_with_metadata(
        &mut self,
        selections: SelectionSet,
        replacement: &str,
        metadata: TransactionMetadata,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        if metadata.source != TransactionSource::Composition {
            self.cancel_composition_before_text_edit()?;
        }

        let selections = selections.normalized();
        self.validate_selection_set(&selections)?;

        let before_selection = selections.clone();
        let replacement_len = replacement.chars().count();
        let replacement = replacement.to_string();

        let mut edits = Vec::new();
        let mut after_selections = Vec::with_capacity(selections.len());
        let mut diff = 0isize;

        for selection in selections.as_slice() {
            let range = selection.range();
            let old_start = range.start().get() as isize;
            let old_end = range.end().get() as isize;
            let new_start = (old_start + diff).max(0) as usize;
            let new_head = CharOffset::new(new_start + replacement_len);

            let is_empty_noop = range.is_empty() && replacement.is_empty();
            let is_same_text_noop =
                !range.is_empty() && self.slice_text(range)?.as_ref() == replacement.as_str();

            if !is_empty_noop && !is_same_text_noop {
                edits.push(Edit::replace(range, replacement.clone()));
                diff += replacement_len as isize - (old_end - old_start);
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

    pub(super) fn replace_single_range_with_metadata(
        &mut self,
        range: TextRange,
        replacement: &str,
        after_selection: SelectionSet,
        metadata: TransactionMetadata,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.validate_range(range)?;
        self.validate_edit_boundary(range.start())?;
        self.validate_edit_boundary(range.end())?;

        if self.slice_text(range)?.as_ref() == replacement {
            self.selection = after_selection;
            return Ok(None);
        }

        let tx = Transaction::from_edits(
            self.version,
            vec![Edit::replace(range, replacement.to_string())],
        )?
        .with_metadata(metadata)
        .with_selection(Some(self.selection.clone()), Some(after_selection));

        self.apply_transaction(tx).map(Some)
    }

    pub(super) fn apply_edit_list(
        &mut self,
        base_version: BufferVersion,
        tx_edits: EditList,
    ) -> EngineResult<(Delta, ChangeSet)> {
        if base_version != self.version {
            return Err(crate::TransactionError::VersionMismatch {
                expected: self.version,
                actual: base_version,
            }
            .into());
        }

        // 1. 预检查：所有 edit 必须在当前旧文本字符坐标系中合法。
        self.validate_edit_list(&tx_edits)?;

        let edits = tx_edits.as_slice().to_vec();
        let old_version = self.version;

        // 2. 在 clone 上应用，确保未来 storage.replace 失败时不污染当前 Buffer。
        let mut new_storage = self.storage.clone();

        let mut reverse_edits = edits;
        reverse_edits.reverse();

        for edit in reverse_edits {
            new_storage.replace(edit.range, &edit.replacement)?;
        }

        // 3. 全部成功后再一次性提交 storage / version。
        self.storage = new_storage;
        self.bump_version()?;

        let new_version = self.version;

        let changeset = ChangeSet::from_edit_list(&tx_edits);

        let delta = Delta {
            old_version,
            new_version,
            edits: tx_edits,
        };

        Ok((delta, changeset))
    }

    /// 递增版本号，溢出时返回错误。
    fn bump_version(&mut self) -> EngineResult<()> {
        self.version = self.version.next().ok_or(EngineError::VersionOverflow)?;
        Ok(())
    }
}
