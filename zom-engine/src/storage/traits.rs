//! 存储 trait 边界：定义 Buffer/Snapshot 需要的只读与可变文本能力。
//!
//! **坐标系唯一真理**：trait 内部以 `ByteOffset` / 字节区间 `TextRange` 为核心；
//! `CharOffset` / `Utf16Position` 是边界投影方法，仅用于公共 API / 外部协议转换。

use std::borrow::Cow;

use crate::{
    ByteOffset, CharOffset, EngineResult, Line, LineEndingStyle, Position, TextRange, Utf16Offset,
    Utf16Position,
};

/// 只读文本视图。
///
/// 不返回裸 `&str` 作为核心抽象——Rope 的全文通常不是一段连续内存。
///
/// **Zero-copy 纪律**：
/// - `chunks(range)` / `all_chunks()` 是**首选 API**，永不分配。
/// - `slice_text` / `text` 在单块场景返回 `Cow::Borrowed`（零拷贝），多块时
///   才物化为 `Cow::Owned`。
/// - `slice_to_string` 明确分配语义，命名让代价显眼。
pub(crate) trait TextRead {
    /// 返回全文。**单块快路径返回 `Cow::Borrowed`，零拷贝**；
    /// 跨多个 chunk 时才退化为 `Cow::Owned`。热路径应改用 `all_chunks`。
    fn text(&self) -> Cow<'_, str>;

    /// 返回指定字节区间的文本。**单块快路径返回 `Cow::Borrowed`，零拷贝**；
    /// 跨多个 chunk 时退化为 `Cow::Owned`。热路径应改用 `chunks(range)`。
    /// 区间端点必须落在 UTF-8 字符边界。
    fn slice_text(&self, range: TextRange) -> EngineResult<Cow<'_, str>>;

    /// 按字节区间流式访问文本块；**永不分配**。
    /// 区间端点必须落在 UTF-8 字符边界。这是 zero-copy 计算的首选 API。
    fn chunks(&self, range: TextRange) -> EngineResult<impl Iterator<Item = &str> + '_>;

    /// 按全文流式访问文本块；**永不分配**。
    #[allow(dead_code)]
    fn all_chunks(&self) -> impl Iterator<Item = &str> + '_;

    /// 物化字节区间为完整 `String`。命名让分配代价显眼。
    /// 调用方应在确认必须连续字符串时才使用。
    #[allow(dead_code)]
    fn slice_to_string(&self, range: TextRange) -> EngineResult<String> {
        let mut out = String::with_capacity(range.len());
        for chunk in self.chunks(range)? {
            out.push_str(chunk);
        }
        Ok(out)
    }

    // ============== 核心 byte 接口（深核必须使用） ==============

    /// 总 UTF-8 字节长度，等价于文本末端的 `ByteOffset`。
    fn len_bytes(&self) -> ByteOffset;

    /// 总行数。空文档也视为 1 行。
    fn line_count(&self) -> usize;

    /// 指定行的起始 ByteOffset。
    fn line_start(&self, line: Line) -> EngineResult<ByteOffset>;

    /// ByteOffset -> line / logical column。
    /// 端点必须是合法字符边界，否则返回 `CoordinateError::InvalidByteBoundary`。
    fn byte_to_position(&self, offset: ByteOffset) -> EngineResult<Position>;

    /// line / logical column -> ByteOffset。
    fn position_to_byte(&self, position: Position) -> EngineResult<ByteOffset>;

    /// 判断 ByteOffset 是否处在合法 grapheme cluster 边界。
    fn is_grapheme_boundary(&self, offset: ByteOffset) -> EngineResult<bool>;

    /// 返回小于当前 offset 的最近 grapheme cluster 边界；开头处返回 0。
    fn previous_grapheme_boundary(&self, offset: ByteOffset) -> EngineResult<ByteOffset>;

    /// 返回大于当前 offset 的最近 grapheme cluster 边界；结尾处返回 len_bytes。
    fn next_grapheme_boundary(&self, offset: ByteOffset) -> EngineResult<ByteOffset>;

    /// 读取指定 byte offset 处的 Unicode scalar。
    /// `offset` 必须是字符边界；越界或不在边界则返回 `None`。
    fn char_at_byte(&self, offset: ByteOffset) -> Option<char>;

    // ============== 边界投影 / 外部协议适配 ==============

    /// 边界投影：CharOffset -> line / logical column。仅公共 API / 外部协议使用。
    fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position>;

    /// 边界投影：line / logical column -> CharOffset。仅公共 API / 外部协议使用。
    fn position_to_char(&self, position: Position) -> EngineResult<CharOffset>;

    /// 边界投影：CharOffset -> UTF-16 行列。仅 LSP 等外部协议。
    fn char_to_utf16_position(&self, offset: CharOffset) -> EngineResult<Utf16Position> {
        self.byte_to_utf16_position(self.char_to_byte(offset)?)
    }

    /// 边界投影：UTF-16 行列 -> CharOffset。仅 LSP 等外部协议。
    fn utf16_position_to_char(&self, position: Utf16Position) -> EngineResult<CharOffset> {
        self.byte_to_char(self.utf16_position_to_byte(position)?)
    }

    /// 边界投影：判断 CharOffset 是否处在合法 grapheme cluster 边界。
    fn is_grapheme_boundary_char(&self, offset: CharOffset) -> EngineResult<bool> {
        let byte = self.char_to_byte(offset)?;
        self.is_grapheme_boundary(byte)
    }

    /// 边界投影：返回小于当前 CharOffset 的最近 grapheme 边界。
    fn previous_grapheme_boundary_char(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        let byte = self.char_to_byte(offset)?;
        let prev_byte = self.previous_grapheme_boundary(byte)?;
        self.byte_to_char(prev_byte)
    }

    /// 边界投影：返回大于当前 CharOffset 的最近 grapheme 边界。
    fn next_grapheme_boundary_char(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        let byte = self.char_to_byte(offset)?;
        let next_byte = self.next_grapheme_boundary(byte)?;
        self.byte_to_char(next_byte)
    }

    /// 边界投影：读取指定 char offset 的 Unicode scalar。
    fn char_at(&self, offset: CharOffset) -> Option<char>;

    /// 总 Unicode scalar 数。仅用于公共 API / 外部协议。
    fn len_chars(&self) -> CharOffset;

    /// 总 UTF-16 code unit 数。仅用于 LSP 等外部协议适配。
    fn len_utf16_cu(&self) -> Utf16Offset;

    /// 边界投影：CharOffset -> ByteOffset。
    fn char_to_byte(&self, offset: CharOffset) -> EngineResult<ByteOffset>;

    /// 边界投影：ByteOffset -> CharOffset。
    fn byte_to_char(&self, offset: ByteOffset) -> EngineResult<CharOffset>;

    /// 边界投影：ByteOffset -> UTF-16 行列。
    fn byte_to_utf16_position(&self, offset: ByteOffset) -> EngineResult<Utf16Position>;

    /// 边界投影：UTF-16 行列 -> ByteOffset。
    fn utf16_position_to_byte(&self, position: Utf16Position) -> EngineResult<ByteOffset>;

    /// 检测文本中实际出现的换行风格。
    fn line_ending_style(&self) -> LineEndingStyle;
}

/// 可跨线程读取的不可变文本快照。
pub(crate) trait TextSnapshot: TextRead + Clone + Send + Sync + 'static {}

impl<T> TextSnapshot for T where T: TextRead + Clone + Send + Sync + 'static {}

/// 可变文本存储后端。
pub(crate) trait TextStorage: TextRead + Clone {
    type Snapshot: TextSnapshot;
    type PreparedReplace;

    fn snapshot(&self) -> Self::Snapshot;

    /// 预检一次替换。`range` 端点必须落在 UTF-8 字符边界。
    ///
    /// 所有可能失败的后端校验、坐标换算和容量预约都必须发生在这里，
    /// 事务管线进入实际文本变异后只能调用不可失败的 `replace_prepared`。
    fn prepare_replace(
        &self,
        range: TextRange,
        replacement: &str,
    ) -> EngineResult<Self::PreparedReplace>;

    /// 执行已经 `prepare_replace` 预检过的替换。
    ///
    /// 调用方必须按旧文本坐标的倒序应用 prepared edits，使每个 prepared range
    /// 在当前文本中仍指向同一段旧文本。该 primitive 不返回 `Result`，从而保护事务
    /// 提交阶段不会在半提交后才发现可恢复错误。
    fn replace_prepared(&mut self, prepared: Self::PreparedReplace, replacement: &str);
}
