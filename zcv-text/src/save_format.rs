//! 保存前的纯文本编辑规划；调用方通过正常事务提交结果。

use crate::{ByteOffset, Edit, Snapshot, TextRange, TextResult, storage::TextRead};

impl Snapshot {
    /// 删除所有行末尾的空格和 Tab，不修改换行符。
    pub fn trailing_whitespace_edits(&self) -> TextResult<Vec<Edit>> {
        let range = TextRange::new(ByteOffset::ZERO, self.len_bytes())?;
        let mut edits = Vec::new();
        let mut trailing_start = None;
        let mut offset = 0;

        for chunk in self.chunks(range)? {
            for &byte in chunk.as_bytes() {
                match byte {
                    b' ' | b'\t' => {
                        trailing_start.get_or_insert(offset);
                    }
                    b'\n' => {
                        if let Some(start) = trailing_start.take() {
                            edits.push(Edit::delete(TextRange::new(
                                ByteOffset::new(start),
                                ByteOffset::new(offset),
                            )?));
                        }
                    }
                    _ => trailing_start = None,
                }
                offset += 1;
            }
        }

        if let Some(start) = trailing_start {
            edits.push(Edit::delete(TextRange::new(
                ByteOffset::new(start),
                self.len_bytes(),
            )?));
        }
        Ok(edits)
    }

    /// 将非空文本末尾连续的 ASCII 空白收成一个 LF；已满足时不产生编辑。
    pub fn final_newline_edit(&self) -> TextResult<Option<Edit>> {
        let len = self.len_bytes();
        if len == ByteOffset::ZERO {
            return Ok(None);
        }

        let range = TextRange::new(ByteOffset::ZERO, len)?;
        let mut content_end = 0;
        let mut offset = 0;
        let mut last_byte = None;
        for chunk in self.chunks(range)? {
            for &byte in chunk.as_bytes() {
                if !byte.is_ascii_whitespace() {
                    content_end = offset + 1;
                }
                last_byte = Some(byte);
                offset += 1;
            }
        }

        if content_end + 1 == len.get() && last_byte == Some(b'\n') {
            return Ok(None);
        }

        Ok(Some(Edit::replace(
            TextRange::new(ByteOffset::new(content_end), len)?,
            "\n",
        )))
    }
}
