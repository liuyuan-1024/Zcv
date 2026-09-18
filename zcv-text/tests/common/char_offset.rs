//! 字符偏移构造器。

use zcv_text::CharOffset;

pub(crate) fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}
