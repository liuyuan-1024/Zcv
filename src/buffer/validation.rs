//! Buffer 校验边界：集中约束 range、selection、edit list、grapheme 与 CRLF 中点等编辑合法性。
//!
//! 本文件只做防线判断并返回明确错误，不修正调用方输入，也不直接改变 Buffer 状态。

use crate::{
    CharOffset, CoordinateError, EditError, EngineResult, SelectionSet, StorageError, TextRange,
    storage::TextStorage, transaction::EditList,
};

use super::{Buffer, coordinates::is_crlf_middle};

impl Buffer {
    pub(in crate::buffer) fn mark_clean_internal(&mut self) {
        self.saved_version = self.version();
        self.saved_snapshot = self.storage.snapshot();
        self.saved_fingerprint = self.saved_snapshot.fingerprint();
    }

    pub(super) fn ensure_writable(&self) -> EngineResult<()> {
        if self.is_read_only() {
            return Err(StorageError::ReadOnly.into());
        }

        Ok(())
    }

    pub(super) fn validate_edit_list(&self, edits: &EditList) -> EngineResult<()> {
        for edit in edits.as_slice() {
            self.validate_range(edit.range)?;
            self.validate_edit_boundary(edit.range.start())?;
            self.validate_edit_boundary(edit.range.end())?;
        }

        Ok(())
    }

    pub(super) fn validate_selection_set(&self, selections: &SelectionSet) -> EngineResult<()> {
        for selection in selections.as_slice() {
            self.validate_selection_boundary(selection.anchor())?;
            self.validate_selection_boundary(selection.head())?;
        }

        Ok(())
    }

    pub(super) fn validate_selection_boundary(&self, offset: CharOffset) -> EngineResult<()> {
        self.validate_edit_boundary(offset)?;
        self.validate_grapheme_boundary(offset)
    }

    /// 校验范围是否合法，超出文本字符长度返回错误。
    pub(super) fn validate_range(&self, range: TextRange) -> EngineResult<()> {
        if range.end() > self.len_chars() {
            return Err(EditError::RangeOutOfBounds { range }.into());
        }

        Ok(())
    }

    /// 校验编辑边界是否合法，超出文本范围或落在 CRLF 中间时返回错误。
    pub(super) fn validate_edit_boundary(&self, offset: CharOffset) -> EngineResult<()> {
        let value = offset.get();
        let len_chars = self.len_chars().get();

        if value > len_chars {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        if is_crlf_middle(&self.storage, offset) {
            return Err(EditError::InvalidBoundary { offset }.into());
        }

        Ok(())
    }
}
