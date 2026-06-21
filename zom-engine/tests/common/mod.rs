//! 集成测试共享 helper——之前 7 个测试文件各写一份，现在收敛到这。
#![allow(dead_code)]

use zom_engine::*;

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

pub fn dcol(value: usize) -> DisplayColumn {
    DisplayColumn::new(value)
}

pub fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

pub fn line_range(start: usize, end: usize) -> LineRange {
    LineRange::new(line(start), line(end)).unwrap()
}

pub fn projected(value: usize) -> ProjectedLineIndex {
    ProjectedLineIndex::new(value)
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

pub fn selection(anchor: usize, head: usize) -> Selection {
    Selection::new(b(anchor), b(head))
}

pub fn caret(offset: usize) -> Selection {
    Selection::caret(b(offset))
}

pub fn set_caret(offset: usize) -> SelectionSet {
    SelectionSet::caret(b(offset))
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
