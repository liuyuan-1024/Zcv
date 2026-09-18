//! 字节偏移与字节范围构造器。

use zcv_text::{ByteOffset, TextRange};

pub(crate) fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

pub(crate) fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}
