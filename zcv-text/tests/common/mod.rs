#![allow(dead_code)]

//! 集成测试共享 helper——之前 7 个测试文件各写一份，现在收敛到这。
//!
//! 各集成测试仅在各自测试 crate 内引入本模块。

use zcv_text::*;

// ---- 坐标构造器 ----

pub(crate) fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

pub(crate) fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

pub(crate) fn line(value: usize) -> Line {
    Line::new(value)
}

pub(crate) fn col(value: usize) -> LogicalColumn {
    LogicalColumn::new(value)
}

pub(crate) fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

pub(crate) fn line_range(start: usize, end: usize) -> LineRange {
    LineRange::new(line(start), line(end)).unwrap()
}

// ---- 缓冲区构造器 ----

pub(crate) fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

pub(crate) fn loaded_buffer(
    bytes: impl AsRef<[u8]>,
    config: BufferConfig,
) -> Result<Buffer, BufferLoadError> {
    Buffer::from_reader(std::io::Cursor::new(bytes.as_ref().to_vec()), config)
}

// ---- 文本读取 ----

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

pub(crate) fn metadata(description: &str) -> TransactionMetadata {
    TransactionMetadata::new(TransactionSource::Programmatic).with_description(description)
}

pub(crate) fn merge_metadata(description: &str) -> TransactionMetadata {
    metadata(description).with_merge_policy(TransactionMergePolicy::MergeWithPrevious)
}
