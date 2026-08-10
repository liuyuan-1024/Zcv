//! 集成测试共享 helper——之前 7 个测试文件各写一份，现在收敛到这。
//!
//! 本模块被多个集成测试文件（tests/*.rs，各自独立编译）以 `mod common` 引入；
//! 每个测试 binary 只用到部分 helper，未用到的 `pub fn` 在该 binary 内报 dead_code，属于共享测试模块的机制噪音，而非死代码。
//! 故在此统一豁免 dead_code。

#![allow(dead_code)]

use zcv_engine::*;

// ---- 坐标构造器 ----

pub fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

pub fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

pub fn line(value: usize) -> Line {
    Line::new(value)
}

pub fn col(value: usize) -> LogicalColumn {
    LogicalColumn::new(value)
}

pub fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

pub fn line_range(start: usize, end: usize) -> LineRange {
    LineRange::new(line(start), line(end)).unwrap()
}

// ---- 缓冲区构造器 ----

pub fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

pub fn loaded_buffer(
    origin: BufferOrigin,
    bytes: impl AsRef<[u8]>,
    config: BufferConfig,
) -> Result<Buffer, BufferLoadError> {
    Buffer::from_reader(
        origin,
        std::io::Cursor::new(bytes.as_ref().to_vec()),
        config,
    )
}

// ---- 文本读取 ----

pub trait FullText {
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

pub fn buffer_text(text: &impl FullText) -> String {
    text.full_text()
}

// ---- 选区构造器 ----

pub fn caret(offset: usize) -> Selection {
    Selection::caret(b(offset))
}

// ---- 事务构造器 ----

pub fn tx(buffer: &Buffer, edits: Vec<Edit>) -> Transaction {
    Transaction::from_edits(buffer.version(), edits).unwrap()
}

pub fn metadata(description: &str) -> TransactionMetadata {
    TransactionMetadata::new(TransactionSource::Programmatic).with_description(description)
}

pub fn merge_metadata(description: &str) -> TransactionMetadata {
    metadata(description).with_merge_policy(TransactionMergePolicy::MergeWithPrevious)
}
