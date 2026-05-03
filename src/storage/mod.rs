//! 文本存储抽象。
//!
//! M1 只提供 crate 内部的最小抽象，避免过早冻结公开 storage API。

mod string_storage;

pub(crate) use string_storage::StringStorage;

use crate::{ByteOffset, EngineResult, TextRange};

pub(crate) trait TextStorage {
    fn text(&self) -> &str;

    fn len_bytes(&self) -> ByteOffset {
        ByteOffset::new(self.text().len())
    }

    fn replace(&mut self, range: TextRange, replacement: &str) -> EngineResult<()>;
}
