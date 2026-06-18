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
use zom_engine::{Buffer, SelectionSet, TextRange, Utf16Offset};

/// 来自 GPUI / AppKit IME 边界的 flat UTF-16 code-unit range。
///
/// 裸 `Range<usize>` 只允许停留在平台 input handler 第一层；
/// 进入编辑器路由后必须带着这个类型，避免把 NSRange / UTF-16 / byte offset 语义混在一起。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImeUtf16Range {
    range: Range<usize>,
}

impl ImeUtf16Range {
    pub(crate) fn new(start: usize, end: usize) -> Result<Self, CommandError> {
        if start > end {
            return Err(CommandError::InvalidArgs(
                "IME UTF-16 range start 大于 end".into(),
            ));
        }

        Ok(Self { range: start..end })
    }

    pub(crate) fn from_gpui_range(range: Range<usize>) -> Result<Self, CommandError> {
        Self::new(range.start, range.end)
    }

    pub(crate) fn into_gpui_range(self) -> Range<usize> {
        self.range
    }

    pub(crate) fn start(&self) -> usize {
        self.range.start
    }

    pub(crate) fn end(&self) -> usize {
        self.range.end
    }

    fn to_text_range(&self, buffer: &Buffer) -> Result<TextRange, CommandError> {
        let start = buffer
            .utf16_cu_to_byte(Utf16Offset::new(self.start()))
            .map_err(|_| CommandError::InvalidArgs("IME range start 越界".into()))?;
        let end = buffer
            .utf16_cu_to_byte(Utf16Offset::new(self.end()))
            .map_err(|_| CommandError::InvalidArgs("IME range end 越界".into()))?;

        TextRange::new(start, end)
            .map_err(|_| CommandError::InvalidArgs("IME range start 大于 end".into()))
    }
}

/// 可被系统输入法查询的编辑目标。
pub(crate) struct ImeQueryTarget<'a> {
    buffer: &'a Buffer,
    selection: &'a SelectionSet,
}

impl<'a> ImeQueryTarget<'a> {
    pub(crate) fn new(buffer: &'a Buffer, selection: &'a SelectionSet) -> Self {
        Self { buffer, selection }
    }

    pub(crate) fn marked_range_utf16(&self) -> Option<ImeUtf16Range> {
        let range = self.buffer.composition()?.range();
        let start = self.buffer.byte_to_utf16_cu(range.start()).ok()?;
        let end = self.buffer.byte_to_utf16_cu(range.end()).ok()?;
        ImeUtf16Range::new(start.get(), end.get()).ok()
    }

    pub(crate) fn selected_range_utf16(&self) -> (ImeUtf16Range, bool) {
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
        (
            ImeUtf16Range::new(start, end)
                .unwrap_or_else(|_| ImeUtf16Range::new(start, start).expect("caret range 合法")),
            primary.is_reversed(),
        )
    }

    pub(crate) fn text_for_range_utf16(&self, range_utf16: ImeUtf16Range) -> Option<String> {
        let range = range_utf16.to_text_range(self.buffer).ok()?;
        self.buffer
            .slice_text(range)
            .ok()
            .map(|slice| slice.as_str().to_string())
    }
}

#[cfg(test)]
mod tests {
    //! preedit 本地 helper 的健壮性测试。buffer 路径（byte_to_utf16_cu /
    //! utf16_cu_to_byte）由 engine 单元测试覆盖。

    use super::*;
    use zom_engine::ByteOffset;

    fn b(value: usize) -> ByteOffset {
        ByteOffset::new(value)
    }

    #[test]
    fn ime_utf16_range_should_convert_to_buffer_byte_range_at_text_boundary() {
        let buffer = Buffer::from_text(
            "a\u{00a0}你b".to_string(),
            zom_engine::BufferConfig::default(),
        )
        .unwrap();
        let range = ImeUtf16Range::new(1, 3).unwrap();

        assert_eq!(
            range.to_text_range(&buffer).unwrap(),
            TextRange::new(b(1), b(6)).unwrap()
        );
    }

    #[test]
    fn ime_utf16_range_should_reject_reversed_external_range() {
        assert!(ImeUtf16Range::new(3, 1).is_err());
    }
}
