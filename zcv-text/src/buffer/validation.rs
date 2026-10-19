//! Buffer 校验边界：集中约束 range、selection、edit list 与 grapheme 等编辑合法性。
//!
//! 本文件只做防线判断并返回明确错误，不修正调用方输入，也不直接改变 Buffer 状态。

use super::Buffer;
use crate::{
    errors::{CoordinateError, EditError, StorageError, TextResult},
    storage::TextRead,
    transaction::EditList,
    types::{ByteOffset, TextRange},
};

impl Buffer {
    pub(super) fn ensure_writable(&self) -> TextResult<()> {
        if self.is_read_only() {
            return Err(StorageError::ReadOnly.into());
        }

        Ok(())
    }

    pub(super) fn validate_edit_list(&self, edits: &EditList) -> TextResult<()> {
        for edit in edits.as_slice() {
            self.validate_range(edit.range())?;
            self.validate_edit_boundary(edit.range().start())?;
            self.validate_edit_boundary(edit.range().end())?;
        }

        Ok(())
    }

    /// 校验范围是否合法（字节区间），端点必须落在 UTF-8 字符边界。
    pub(super) fn validate_range(&self, range: TextRange) -> TextResult<()> {
        let len_bytes = self.storage.len_bytes();
        if range.end() > len_bytes {
            return Err(EditError::RangeOutOfBounds { range }.into());
        }
        if self.storage.is_grapheme_boundary(range.start()).is_err() {
            // 不构成字符边界即视为越界
            return Err(EditError::RangeOutOfBounds { range }.into());
        }
        Ok(())
    }

    /// 校验编辑边界（字节偏移）是否合法：不越界，且是 UTF-8 字符边界。
    pub(super) fn validate_edit_boundary(&self, offset: ByteOffset) -> TextResult<()> {
        let value = offset.get();
        let len_bytes = self.storage.len_bytes().get();

        if value > len_bytes {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        // 校验是否落在 UTF-8 字符边界：byte_to_char 对非字符边界返回 InvalidByteBoundary
        if value < len_bytes && self.storage.byte_to_char(offset).is_err() {
            return Err(CoordinateError::InvalidByteBoundary(offset).into());
        }

        Ok(())
    }
}
