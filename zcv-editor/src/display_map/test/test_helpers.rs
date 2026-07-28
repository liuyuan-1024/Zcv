//! DisplayMap 测试辅助函数。

use zcv_engine::{
    Buffer, BufferConfig, ByteOffset, Edit, Line, LineRange, LogicalColumn, TextRange, Transaction,
};

use super::super::projection::ProjectedLineIndex;

pub(super) fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

pub(super) fn line(value: usize) -> Line {
    Line::new(value)
}

pub(super) fn col(value: usize) -> LogicalColumn {
    LogicalColumn::new(value)
}

pub(super) fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

pub(super) fn line_range(start: usize, end: usize) -> LineRange {
    LineRange::new(line(start), line(end)).unwrap()
}

pub(super) fn projected(value: usize) -> ProjectedLineIndex {
    ProjectedLineIndex::new(value)
}

pub(super) fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

pub(super) fn tx(buffer: &Buffer, edits: Vec<Edit>) -> Transaction {
    Transaction::from_edits(buffer.version(), edits).unwrap()
}
