//! 文本存储抽象。
//!
//! M3.5 起，TextStorage 的编辑入口使用 CharOffset / TextRange(char range)，
//! 为 M4 接入 ropey 做准备。

mod string_storage;

pub(crate) use string_storage::StringStorage;

use crate::{CharOffset, EngineResult, TextRange};

pub(crate) trait TextStorage {
    fn text(&self) -> &str;

    fn len_chars(&self) -> CharOffset {
        CharOffset::new(self.text().chars().count())
    }

    fn replace(&mut self, range: TextRange, replacement: &str) -> EngineResult<()>;
}
