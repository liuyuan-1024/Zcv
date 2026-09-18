//! 读取 Buffer 与 Snapshot 全文的测试 helper。

use zcv_text::{Buffer, ByteOffset, Snapshot};

pub(crate) trait FullText {
    fn full_text(&self) -> String;
}

impl FullText for Buffer {
    fn full_text(&self) -> String {
        self.slice_byte_range(ByteOffset::ZERO, self.len_bytes())
            .unwrap()
            .into_text()
            .into_owned()
    }
}

impl FullText for Snapshot {
    fn full_text(&self) -> String {
        self.slice_byte_range(ByteOffset::ZERO, self.len_bytes())
            .unwrap()
            .into_text()
            .into_owned()
    }
}

pub(crate) fn buffer_text(text: &impl FullText) -> String {
    text.full_text()
}
