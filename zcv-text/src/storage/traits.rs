//! 存储 trait 边界：定义 Buffer/Snapshot 需要的只读与可变文本能力。
//!
//! **坐标系唯一真理**：trait 内部以 `ByteOffset` / 字节区间 `TextRange` 为核心；
//! `CharOffset` / `Utf16Position` 是边界投影方法，仅用于公共 API / 外部协议转换。

use std::borrow::Cow;

use crate::{
    errors::TextResult,
    types::{
        ByteOffset, CharOffset, Line, LineEndingStyle, Position, TextRange, Utf16Offset,
        Utf16Position,
    },
};

/// 只读文本视图。
///
/// 不返回裸 `&str` 作为核心抽象——Rope 的全文通常不是一段连续内存。
///
/// **Zero-copy 纪律**：
/// - `chunks(range)` 是**首选 API**，永不分配。
/// - `slice_text` 在单块场景返回 `Cow::Borrowed`（零拷贝），多块时才物化为
///   `Cow::Owned`。
/// - `slice_to_string` 明确分配语义，命名让代价显眼。
pub(crate) trait TextRead {
    /// 返回指定字节区间的文本。**单块快路径返回 `Cow::Borrowed`，零拷贝**；
    /// 跨多个 chunk 时退化为 `Cow::Owned`。热路径应改用 `chunks(range)`。
    /// 区间端点必须落在 UTF-8 字符边界。
    fn slice_text(&self, range: TextRange) -> TextResult<Cow<'_, str>>;

    /// 按字节区间流式访问文本块；**永不分配**。
    /// 区间端点必须落在 UTF-8 字符边界。这是 zero-copy 计算的首选 API。
    fn chunks(&self, range: TextRange) -> TextResult<impl Iterator<Item = &str> + '_>;

    /// 物化字节区间为完整 `String`。命名让分配代价显眼。
    /// 调用方应在确认必须连续字符串时才使用。
    fn slice_to_string(&self, range: TextRange) -> TextResult<String> {
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
    fn line_start(&self, line: Line) -> TextResult<ByteOffset>;

    /// ByteOffset -> line / logical column。
    /// 端点必须是合法字符边界，否则返回 `CoordinateError::InvalidByteBoundary`。
    fn byte_to_position(&self, offset: ByteOffset) -> TextResult<Position>;

    /// ByteOffset -> 仅行号。语义和约束与 `byte_to_position` 相同，只省掉列计算。
    /// 默认实现走 `byte_to_position`；后端可以 override 提供更快的实现。
    fn byte_to_line(&self, offset: ByteOffset) -> TextResult<Line> {
        Ok(self.byte_to_position(offset)?.line())
    }

    /// line / logical column -> ByteOffset。
    fn position_to_byte(&self, position: Position) -> TextResult<ByteOffset>;

    /// 判断 ByteOffset 是否处在合法 grapheme cluster 边界。
    fn is_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<bool>;

    /// 返回小于当前 offset 的最近 grapheme cluster 边界；开头处返回 0。
    fn previous_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset>;

    /// 返回大于当前 offset 的最近 grapheme cluster 边界；结尾处返回 len_bytes。
    fn next_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset>;

    /// 读取指定 byte offset 处的 Unicode scalar。
    /// `offset` 必须是字符边界；越界或不在边界则返回 `None`。
    fn char_at_byte(&self, offset: ByteOffset) -> Option<char>;

    // ============== 边界投影 / 外部协议适配 ==============

    /// 边界投影：CharOffset -> line / logical column。仅公共 API / 外部协议使用。
    fn char_to_position(&self, offset: CharOffset) -> TextResult<Position>;

    /// 边界投影：line / logical column -> CharOffset。仅公共 API / 外部协议使用。
    fn position_to_char(&self, position: Position) -> TextResult<CharOffset>;

    /// 边界投影：CharOffset -> UTF-16 行列。仅 LSP 等外部协议。
    fn char_to_utf16_position(&self, offset: CharOffset) -> TextResult<Utf16Position> {
        self.byte_to_utf16_position(self.char_to_byte(offset)?)
    }

    /// 边界投影：UTF-16 行列 -> CharOffset。仅 LSP 等外部协议。
    fn utf16_position_to_char(&self, position: Utf16Position) -> TextResult<CharOffset> {
        self.byte_to_char(self.utf16_position_to_byte(position)?)
    }

    /// 边界投影：判断 CharOffset 是否处在合法 grapheme cluster 边界。
    fn is_grapheme_boundary_char(&self, offset: CharOffset) -> TextResult<bool> {
        let byte = self.char_to_byte(offset)?;
        self.is_grapheme_boundary(byte)
    }

    /// 边界投影：返回小于当前 CharOffset 的最近 grapheme 边界。
    fn previous_grapheme_boundary_char(&self, offset: CharOffset) -> TextResult<CharOffset> {
        let byte = self.char_to_byte(offset)?;
        let prev_byte = self.previous_grapheme_boundary(byte)?;
        self.byte_to_char(prev_byte)
    }

    /// 边界投影：返回大于当前 CharOffset 的最近 grapheme 边界。
    fn next_grapheme_boundary_char(&self, offset: CharOffset) -> TextResult<CharOffset> {
        let byte = self.char_to_byte(offset)?;
        let next_byte = self.next_grapheme_boundary(byte)?;
        self.byte_to_char(next_byte)
    }

    /// 边界投影：读取指定 char offset 的 Unicode scalar。
    fn char_at(&self, offset: CharOffset) -> Option<char>;

    /// 总 Unicode scalar 数。仅用于公共 API / 外部协议。
    fn len_chars(&self) -> CharOffset;

    /// 边界投影：CharOffset -> ByteOffset。
    fn char_to_byte(&self, offset: CharOffset) -> TextResult<ByteOffset>;

    /// 边界投影：ByteOffset -> CharOffset。
    fn byte_to_char(&self, offset: ByteOffset) -> TextResult<CharOffset>;

    /// 边界投影：ByteOffset -> UTF-16 行列。
    fn byte_to_utf16_position(&self, offset: ByteOffset) -> TextResult<Utf16Position>;

    /// 边界投影：UTF-16 行列 -> ByteOffset。
    fn utf16_position_to_byte(&self, position: Utf16Position) -> TextResult<ByteOffset>;

    /// 边界投影：ByteOffset -> 全文 flat UTF-16 code unit 偏移。
    ///
    /// 与 `byte_to_utf16_position` 的区别：本方法返回从文本起点起的累计 UTF-16
    /// code unit 数，对应 NSTextInputClient / Win32 TSF 等系统 IME 的「flat
    /// utf-16 offset」语义。端点必须落在 UTF-8 字符边界，否则返回
    /// `CoordinateError::InvalidByteBoundary`。
    fn byte_to_utf16_cu(&self, offset: ByteOffset) -> TextResult<Utf16Offset>;

    /// 边界投影：全文 flat UTF-16 code unit 偏移 -> ByteOffset。
    ///
    /// 端点必须落在 UTF-16 code unit 边界（不能落在 surrogate pair 中间），
    /// 否则返回 `CoordinateError::InvalidUtf16Boundary`。
    fn utf16_cu_to_byte(&self, offset: Utf16Offset) -> TextResult<ByteOffset>;

    /// 检测文本中实际出现的换行风格。
    fn line_ending_style(&self) -> LineEndingStyle;
}

/// 可跨线程读取的不可变文本快照。
pub(crate) trait TextSnapshot: TextRead + Clone + Send + Sync + 'static {}

impl<T> TextSnapshot for T where T: TextRead + Clone + Send + Sync + 'static {}

/// Buffer / Snapshot 共用的坐标门面批量转发宏。
///
/// 两份门面（`Buffer::storage` 与 `Snapshot::storage` 都实现 `TextRead`）的方法签名与转发体逐行相同。
/// 在这里把签名与实现写一次，两个 impl 各自展开，行为修复只需改这一处。
macro_rules! text_coordinate_gateway {
    () => {
        /// 总 Unicode scalar 数（边界投影）。
        pub fn len_chars(&self) -> $crate::CharOffset {
            self.storage.len_chars()
        }

        /// 文本 UTF-8 字节末端位置；等价于全文末尾的 `ByteOffset`。
        pub fn len_bytes(&self) -> $crate::ByteOffset {
            self.storage.len_bytes()
        }

        /// 总行数。空文档也视为 1 行。
        pub fn line_count(&self) -> usize {
            self.storage.line_count()
        }

        /// 指定行的起始 ByteOffset（深核接口）。
        pub fn line_start_byte(
            &self,
            line: $crate::Line,
        ) -> $crate::TextResult<$crate::ByteOffset> {
            self.storage.line_start(line)
        }

        /// 指定行的起始 CharOffset（边界投影）。
        pub fn line_start(&self, line: $crate::Line) -> $crate::TextResult<$crate::CharOffset> {
            let byte = self.storage.line_start(line)?;
            self.storage.byte_to_char(byte)
        }

        /// ByteOffset -> line / logical column。
        pub fn byte_to_position(
            &self,
            offset: $crate::ByteOffset,
        ) -> $crate::TextResult<$crate::Position> {
            self.storage.byte_to_position(offset)
        }

        /// `byte_to_position` 的省列变体（宿主投影几何只关心行号时走这条路径）。
        pub fn byte_to_line(&self, offset: $crate::ByteOffset) -> $crate::TextResult<$crate::Line> {
            self.storage.byte_to_line(offset)
        }

        /// line / logical column -> ByteOffset。
        pub fn position_to_byte(
            &self,
            position: $crate::Position,
        ) -> $crate::TextResult<$crate::ByteOffset> {
            self.storage.position_to_byte(position)
        }

        /// CharOffset -> line / logical column（边界投影）。
        pub fn char_to_position(
            &self,
            offset: $crate::CharOffset,
        ) -> $crate::TextResult<$crate::Position> {
            self.storage.char_to_position(offset)
        }

        /// line / logical column -> CharOffset（边界投影）。
        pub fn position_to_char(
            &self,
            position: $crate::Position,
        ) -> $crate::TextResult<$crate::CharOffset> {
            self.storage.position_to_char(position)
        }

        /// CharOffset -> ByteOffset（边界投影）。
        pub fn char_to_byte(
            &self,
            offset: $crate::CharOffset,
        ) -> $crate::TextResult<$crate::ByteOffset> {
            self.storage.char_to_byte(offset)
        }

        /// ByteOffset -> CharOffset（边界投影）。
        pub fn byte_to_char(
            &self,
            offset: $crate::ByteOffset,
        ) -> $crate::TextResult<$crate::CharOffset> {
            self.storage.byte_to_char(offset)
        }

        /// CharOffset -> UTF-16 行列（LSP 等外部协议）。
        pub fn char_to_utf16_position(
            &self,
            offset: $crate::CharOffset,
        ) -> $crate::TextResult<$crate::Utf16Position> {
            self.storage.char_to_utf16_position(offset)
        }

        /// UTF-16 行列 -> CharOffset（LSP 等外部协议）。
        pub fn utf16_position_to_char(
            &self,
            position: $crate::Utf16Position,
        ) -> $crate::TextResult<$crate::CharOffset> {
            self.storage.utf16_position_to_char(position)
        }

        /// ByteOffset -> UTF-16 行列（LSP 等外部协议）。
        pub fn byte_to_utf16_position(
            &self,
            offset: $crate::ByteOffset,
        ) -> $crate::TextResult<$crate::Utf16Position> {
            self.storage.byte_to_utf16_position(offset)
        }

        /// UTF-16 行列 -> ByteOffset（LSP 等外部协议）。
        pub fn utf16_position_to_byte(
            &self,
            position: $crate::Utf16Position,
        ) -> $crate::TextResult<$crate::ByteOffset> {
            self.storage.utf16_position_to_byte(position)
        }

        /// 全文 flat UTF-16 code unit 偏移：byte → utf16 cu。
        ///
        /// 给系统 IME 的扁平 UTF-16 offset 语义用，不要走 `byte_to_utf16_position`
        /// （那是 LSP 协议的行/列）。
        pub fn byte_to_utf16_cu(
            &self,
            offset: $crate::ByteOffset,
        ) -> $crate::TextResult<$crate::Utf16Offset> {
            self.storage.byte_to_utf16_cu(offset)
        }

        /// 全文 flat UTF-16 code unit 偏移：utf16 cu → byte。
        pub fn utf16_cu_to_byte(
            &self,
            offset: $crate::Utf16Offset,
        ) -> $crate::TextResult<$crate::ByteOffset> {
            self.storage.utf16_cu_to_byte(offset)
        }

        /// 判断 ByteOffset 是否处在合法 grapheme cluster 边界。
        pub fn is_grapheme_boundary_byte(
            &self,
            offset: $crate::ByteOffset,
        ) -> $crate::TextResult<bool> {
            self.storage.is_grapheme_boundary(offset)
        }

        /// 判断 CharOffset 是否处在合法 grapheme cluster 边界。
        pub fn is_grapheme_boundary(&self, offset: $crate::CharOffset) -> $crate::TextResult<bool> {
            self.storage.is_grapheme_boundary_char(offset)
        }

        /// 校验 CharOffset 是否处在合法 grapheme cluster 边界，否则返回错误。
        pub fn validate_grapheme_boundary(
            &self,
            offset: $crate::CharOffset,
        ) -> $crate::TextResult<()> {
            if self.storage.is_grapheme_boundary_char(offset)? {
                Ok(())
            } else {
                let byte = self.storage.char_to_byte(offset)?;
                Err($crate::CoordinateError::InvalidGraphemeBoundary(byte).into())
            }
        }

        /// 返回小于当前 CharOffset 的最近 grapheme cluster 边界。
        pub fn previous_grapheme_boundary(
            &self,
            offset: $crate::CharOffset,
        ) -> $crate::TextResult<$crate::CharOffset> {
            self.storage.previous_grapheme_boundary_char(offset)
        }

        /// 返回大于当前 CharOffset 的最近 grapheme cluster 边界。
        pub fn next_grapheme_boundary(
            &self,
            offset: $crate::CharOffset,
        ) -> $crate::TextResult<$crate::CharOffset> {
            self.storage.next_grapheme_boundary_char(offset)
        }

        /// 检测文本中实际出现的换行风格。
        pub fn line_ending_style(&self) -> $crate::LineEndingStyle {
            self.storage.line_ending_style()
        }
    };
}
pub(crate) use text_coordinate_gateway;

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
    ) -> TextResult<Self::PreparedReplace>;

    /// 执行已经 `prepare_replace` 预检过的替换。
    ///
    /// 调用方必须按旧文本坐标的倒序应用 prepared edits，使每个 prepared range
    /// 在当前文本中仍指向同一段旧文本。该 primitive 不返回 `Result`，从而保护事务
    /// 提交阶段不会在半提交后才发现可恢复错误。
    fn replace_prepared(&mut self, prepared: Self::PreparedReplace, replacement: &str);
}
