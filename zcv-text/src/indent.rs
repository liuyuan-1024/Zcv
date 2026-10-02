//! 文本行首的 Tab、空格与空白状态。

use crate::{ByteOffset, Snapshot, TextRange, TextRead, TextResult};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineIndent {
    pub tabs: usize,
    pub spaces: usize,
    pub line_blank: bool,
}

impl LineIndent {
    pub fn is_line_empty(self) -> bool {
        self.tabs == 0 && self.spaces == 0 && self.line_blank
    }

    pub fn is_line_blank(self) -> bool {
        self.line_blank
    }

    pub fn raw_len(self) -> usize {
        self.tabs + self.spaces
    }

    pub fn len(self, tab_size: usize) -> usize {
        self.tabs * tab_size + self.spaces
    }
}

impl Snapshot {
    /// 只扫描指定源范围的行首；范围可以被 excerpt 边界截断。
    pub fn line_indent_in_range(
        &self,
        start: ByteOffset,
        end: ByteOffset,
    ) -> TextResult<LineIndent> {
        let range = TextRange::new(start, end)?;
        let mut indent = LineIndent {
            line_blank: true,
            ..LineIndent::default()
        };
        for chunk in self.chunks(range)? {
            for byte in chunk.bytes() {
                match byte {
                    b'\t' => indent.tabs += 1,
                    b' ' => indent.spaces += 1,
                    b'\r' | b'\n' => return Ok(indent),
                    _ => {
                        indent.line_blank = false;
                        return Ok(indent);
                    }
                }
            }
        }
        Ok(indent)
    }
}
