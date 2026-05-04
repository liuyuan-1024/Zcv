use super::TextStorage;
use crate::{CharOffset, CoordinateError, EngineResult, TextRange};

/// M1/M3.5 参考文本后端。
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
        let start = char_to_byte_index(&self.text, range.start())?;
        let end = char_to_byte_index(&self.text, range.end())?;

        self.text.replace_range(start..end, replacement);

        Ok(())
    }
}

fn char_to_byte_index(text: &str, offset: CharOffset) -> EngineResult<usize> {
    let char_offset = offset.get();
    let len_chars = text.chars().count();

    if char_offset > len_chars {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    if char_offset == len_chars {
        return Ok(text.len());
    }

    text.char_indices()
        .nth(char_offset)
        .map(|(byte_idx, _)| byte_idx)
        .ok_or_else(|| CoordinateError::OutOfBounds(offset).into())
}
