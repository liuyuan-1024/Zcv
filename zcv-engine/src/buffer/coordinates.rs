//! Buffer 坐标门面：把存储层 byte 深核接口投影为编辑器需要的边界（char / UTF-16 / line）API。
//!
//! 本文件绑定 BufferConfig 并处理 CRLF、grapheme 等策略，不直接修改文本或历史。
//! 坐标方法由 [`crate::storage::text_coordinate_gateway`] 宏统一生成，与 `Snapshot` 共用一份实现。

use super::Buffer;
use crate::{CharOffset, storage::TextRead, storage::text_coordinate_gateway};

impl Buffer {
    text_coordinate_gateway!();
}

pub(super) fn is_crlf_middle<T: TextRead>(storage: &T, offset: CharOffset) -> bool {
    let value = offset.get();

    value > 0
        && value < storage.len_chars().get()
        && storage.char_at(CharOffset::new(value - 1)) == Some('\r')
        && storage.char_at(offset) == Some('\n')
}
