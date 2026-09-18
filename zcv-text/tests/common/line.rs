//! 行号构造器。

use zcv_text::Line;

pub(crate) fn line(value: usize) -> Line {
    Line::new(value)
}
