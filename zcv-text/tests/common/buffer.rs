//! 测试缓冲区构造器。

use zcv_text::{Buffer, BufferConfig};

pub(crate) fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}
