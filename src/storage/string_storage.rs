use super::TextStorage;
use crate::{EngineResult, TextRange};

/// M1 参考文本后端。
///
/// 这是语义验证用后端，不是最终高性能后端。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StringStorage {
    text: String,
}

impl StringStorage {
    pub(crate) fn new(text: String) -> Self {
        Self { text }
    }
}

impl TextStorage for StringStorage {
    fn text(&self) -> &str {
        &self.text
    }

    fn replace(&mut self, range: TextRange, replacement: &str) -> EngineResult<()> {
        let start = range.start().get();
        let end = range.end().get();

        // 公共 Buffer API 会先校验 range、UTF-8 边界与 CRLF 边界。
        self.text.replace_range(start..end, replacement);

        Ok(())
    }
}
