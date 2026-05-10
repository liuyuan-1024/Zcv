//! 存储 trait 边界：定义 Buffer/Snapshot 需要的只读与可变文本能力。
//!
//! Trait 使用 CharOffset/TextRange 作为编辑坐标，不暴露 ropey 或连续字符串假设。

use std::borrow::Cow;

use crate::{
    ByteOffset, CharOffset, EngineResult, Line, LineEndingStyle, Position, TextRange, Utf16Position,
};

/// 只读文本视图。
///
/// 不返回裸 `&str` 作为核心抽象——Rope 的全文通常不是一段连续内存。
pub(crate) trait TextRead {
    /// 返回全文。
    ///
    /// `RopeyStorage` 会按需拼接为 owned String；测试参考模型可在测试模块内自行借用返回。
    /// 热路径应优先使用 `slice_text` / metrics / line API，而不是全文读取。
    fn text(&self) -> Cow<'_, str>;

    /// 返回指定字符区间的文本。
    fn slice_text(&self, range: TextRange) -> EngineResult<Cow<'_, str>>;

    /// 总 UTF-8 字节数。
    fn len_bytes(&self) -> usize;

    /// 总 Unicode scalar 数，这是核心编辑坐标单位。
    fn len_chars(&self) -> CharOffset;

    /// 总 UTF-16 code unit 数，为后续 LSP 坐标适配准备。
    fn len_utf16_cu(&self) -> usize;

    /// 总行数。空文档也视为 1 行。
    fn line_count(&self) -> usize;

    /// 指定行的起始 CharOffset。
    fn line_start(&self, line: Line) -> EngineResult<CharOffset>;

    /// CharOffset -> line / logical column。
    fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position>;

    /// line / logical column -> CharOffset。
    fn position_to_char(&self, position: Position) -> EngineResult<CharOffset>;

    /// UTF-8 byte offset -> CharOffset。
    fn byte_to_char(&self, offset: ByteOffset) -> EngineResult<CharOffset>;

    /// CharOffset -> UTF-8 byte offset。
    fn char_to_byte(&self, offset: CharOffset) -> EngineResult<ByteOffset>;

    /// CharOffset -> UTF-16 行列位置。
    fn char_to_utf16_position(&self, offset: CharOffset) -> EngineResult<Utf16Position>;

    /// UTF-16 行列位置 -> CharOffset。
    fn utf16_position_to_char(&self, position: Utf16Position) -> EngineResult<CharOffset>;

    /// UTF-8 byte offset -> UTF-16 行列位置。
    fn byte_to_utf16_position(&self, offset: ByteOffset) -> EngineResult<Utf16Position> {
        self.char_to_utf16_position(self.byte_to_char(offset)?)
    }

    /// UTF-16 行列位置 -> UTF-8 byte offset。
    fn utf16_position_to_byte(&self, position: Utf16Position) -> EngineResult<ByteOffset> {
        self.char_to_byte(self.utf16_position_to_char(position)?)
    }

    /// 判断 CharOffset 是否处在合法 grapheme cluster 边界。
    fn is_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<bool>;

    /// 返回小于当前 offset 的最近 grapheme cluster 边界；开头处返回 0。
    fn previous_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset>;

    /// 返回大于当前 offset 的最近 grapheme cluster 边界；结尾处返回 len_chars。
    fn next_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset>;

    /// 检测文本中实际出现的换行风格。
    fn line_ending_style(&self) -> LineEndingStyle;

    /// 读取指定字符。越界返回 None。
    fn char_at(&self, offset: CharOffset) -> Option<char>;
}

/// 可跨线程读取的不可变文本快照。
pub(crate) trait TextSnapshot: TextRead + Clone + Send + Sync + 'static {}

impl<T> TextSnapshot for T where T: TextRead + Clone + Send + Sync + 'static {}

/// 可变文本存储后端。
pub(crate) trait TextStorage: TextRead + Clone {
    type Snapshot: TextSnapshot;

    fn snapshot(&self) -> Self::Snapshot;

    fn replace(&mut self, range: TextRange, replacement: &str) -> EngineResult<()>;
}
