//! Markdown 预览的选择领域：稳定选择坐标空间、选择手势状态机与当前帧命中几何。
//!
//! 三项职责按权威与派生分离：
//! - [`SelectableText`] 是选择坐标空间，由块树在 refresh 时投影，两次 refresh 之间稳定；
//! 公式图片加载、代码高亮完成、窗口尺寸变化只改变呈现与几何，不改变选择坐标。
//! - [`SelectionModel`] 拥有选择状态与鼠标手势，语义对齐 Editor 的 begin/update/end
//! selection：单击定位、双击选词、三击选块、四击全选，Shift+点击按上次粒度扩展。
//! - [`SelectionLayout`] 是当前帧的命中几何，由渲染逐帧重建，只服务命中测试与选区绘制，不是位置权威。

use std::{borrow::Cow, ops::Range};

use gpui::{Bounds, Pixels, Point, TextLayout, point};
use unicode_segmentation::UnicodeSegmentation;
use zcv_text::{
    ByteOffset, CharOffset, CoordinateError, Line, LineEndingStyle, LogicalColumn, Position,
    TextRange, TextRead, TextResult, Utf16Offset, WordBoundaryPolicy, is_inside_word_in_range,
    surrounding_word_in_range,
};

use crate::document::{Block, BlockKind, Inline};

/// 选择坐标空间：块树投影出的「复制文本」序列。
///
/// 投影与渲染共享同一套遍历顺序：块与块、代码行与代码行之间以 `\n` 分隔，
/// 表格单元格之间以 `\t` 分隔；行内公式直接使用其 LaTeX 源码。
/// 因此复制结果就是渲染后的纯文本，无需事后替换；
/// 渲染期的公式占位只影响布局，通过 [`SelectionLayout`] 的占位映射换算回本空间。
pub(crate) struct SelectableText {
    text: String,
    /// 每个可选文本片段（段落、标题、代码行、表格单元格）的区间，按文档顺序。
    /// 渲染按相同顺序为每个片段注册命中几何。
    segments: Vec<Range<usize>>,
    /// 每个非空块（含嵌套块）的区间，按文档顺序；区间含块尾换行符。
    blocks: Vec<Range<usize>>,
}

impl SelectableText {
    pub(crate) fn from_blocks(blocks: &[Block]) -> Self {
        let mut projection = Projection::default();
        for block in blocks {
            projection.project_block(block);
        }
        // 块区间延伸到文档顺序下一个块的内容起点，使每块包含自己的尾部换行符；
        // 三击复制整块时保持块边界。
        //
        // 容器块（列表、引用）在子块之后记录，其内容起点早于子块终点；
        // 查找下一个块时按「内容起点不早于本块内容终点」过滤即可跳过祖先容器。
        let contents = projection.block_contents;
        let mut block_ranges = Vec::with_capacity(contents.len());
        for (index, range) in contents.iter().enumerate() {
            let end = contents[index + 1..]
                .iter()
                .find(|next| next.start >= range.end)
                .map_or(projection.text.len(), |next| next.start);
            block_ranges.push(range.start..end);
        }
        Self {
            text: projection.text,
            segments: projection.segments,
            blocks: block_ranges,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.text.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn segments(&self) -> &[Range<usize>] {
        &self.segments
    }

    /// 偏移所在的词范围；空范围（换行间隙、空隙）返回 `None`。
    fn surrounding_word(&self, offset: usize) -> Option<Range<usize>> {
        let char_offset = self.byte_to_char(ByteOffset::new(offset)).ok()?;
        let (start, end) = surrounding_word_in_range(
            self,
            WordBoundaryPolicy::default(),
            char_offset,
            CharOffset::ZERO..self.len_chars(),
        )
        .ok()?;
        let start = self.char_to_byte(start).ok()?.get();
        let end = self.char_to_byte(end).ok()?.get();
        (start < end).then_some(start..end)
    }

    /// 偏移两侧是否属于同一词（拖拽词粒度的吸附判定）。
    fn is_inside_word(&self, offset: usize) -> bool {
        let Ok(char_offset) = self.byte_to_char(ByteOffset::new(offset)) else {
            return false;
        };
        is_inside_word_in_range(
            self,
            WordBoundaryPolicy::default(),
            char_offset,
            CharOffset::ZERO..self.len_chars(),
        )
        .unwrap_or(false)
    }

    /// 偏移所在的最内层块区间；命中块间换行符时归属前一块。
    fn block_range(&self, offset: usize) -> Option<Range<usize>> {
        self.blocks
            .iter()
            .filter(|range| range.contains(&offset))
            .min_by_key(|range| range.len())
            .cloned()
    }

    fn validate_range(&self, range: TextRange) -> TextResult<()> {
        let start = range.start().get();
        let end = range.end().get();
        if end > self.text.len() {
            return Err(CoordinateError::OutOfBounds(range.end()).into());
        }
        if !self.text.is_char_boundary(start) {
            return Err(CoordinateError::InvalidByteBoundary(range.start()).into());
        }
        if !self.text.is_char_boundary(end) {
            return Err(CoordinateError::InvalidByteBoundary(range.end()).into());
        }
        Ok(())
    }

    fn validate_offset(&self, offset: ByteOffset) -> TextResult<usize> {
        let byte = offset.get();
        if byte > self.text.len() {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }
        if !self.text.is_char_boundary(byte) {
            return Err(CoordinateError::InvalidByteBoundary(offset).into());
        }
        Ok(byte)
    }
}

/// 块树到选择坐标空间的投影状态。
#[derive(Default)]
struct Projection {
    text: String,
    segments: Vec<Range<usize>>,
    block_contents: Vec<Range<usize>>,
}

impl Projection {
    /// 块间分隔：文本非空且不以换行结尾时补一个换行，与渲染的块顺序一致。
    fn begin_block(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    fn project_block(&mut self, block: &Block) {
        self.begin_block();
        let content_start = self.text.len();
        match &block.kind {
            BlockKind::Heading { content, .. } | BlockKind::Paragraph(content) => {
                self.push_inline_segment(content);
            }
            BlockKind::Code { text, .. } => {
                let mut has_line = false;
                for line in crate::document::code_lines(text) {
                    if has_line {
                        self.text.push('\n');
                    }
                    self.push_segment(line);
                    has_line = true;
                }
            }
            BlockKind::Quote(children) => {
                for child in children {
                    self.project_block(child);
                }
            }
            BlockKind::List { items, .. } => {
                for item in items {
                    for child in item {
                        self.project_block(child);
                    }
                }
            }
            BlockKind::Table { header, rows, .. } => {
                self.project_table_row(header);
                for row in rows {
                    self.project_table_row(row);
                }
            }
            BlockKind::Image { .. }
            | BlockKind::Math { .. }
            | BlockKind::Mermaid { .. }
            | BlockKind::Rule => {}
        }
        let content_end = self.text.len();
        if content_start < content_end {
            self.block_contents.push(content_start..content_end);
        }
    }

    fn project_table_row(&mut self, cells: &[Vec<Inline>]) {
        self.begin_block();
        for (index, cell) in cells.iter().enumerate() {
            if index > 0 {
                self.text.push('\t');
            }
            self.push_inline_segment(cell);
        }
    }

    fn push_inline_segment(&mut self, content: &[Inline]) {
        let start = self.text.len();
        for inline in content {
            self.text.push_str(&inline.text);
        }
        self.finish_segment(start);
    }

    fn push_segment(&mut self, line: &str) {
        let start = self.text.len();
        self.text.push_str(line);
        self.finish_segment(start);
    }

    fn finish_segment(&mut self, start: usize) {
        if start < self.text.len() {
            self.segments.push(start..self.text.len());
        }
    }
}

impl SelectableText {
    /// 键盘左移：offset 的前一个字素边界；越界时保持原位。
    pub(crate) fn prev_grapheme_offset(&self, offset: usize) -> usize {
        self.previous_grapheme_boundary(ByteOffset::new(offset))
            .map_or(offset, |boundary| boundary.get())
    }

    /// 键盘右移：offset 的后一个字素边界；越界时保持原位。
    pub(crate) fn next_grapheme_offset(&self, offset: usize) -> usize {
        self.next_grapheme_boundary(ByteOffset::new(offset))
            .map_or(offset, |boundary| boundary.get())
    }
}

// SelectableText 是一段连续字符串，为其适配 TextRead 以复用 zcv-text 的词边界与 grapheme 语义，保证预览双击选词与 Editor 行为一致。
impl TextRead for SelectableText {
    fn slice_text(&self, range: TextRange) -> TextResult<Cow<'_, str>> {
        self.validate_range(range)?;
        Ok(Cow::Borrowed(
            &self.text[range.start().get()..range.end().get()],
        ))
    }

    fn chunks(&self, range: TextRange) -> TextResult<impl Iterator<Item = &str> + '_> {
        self.validate_range(range)?;
        let slice = &self.text[range.start().get()..range.end().get()];
        Ok(std::iter::once(slice))
    }

    fn len_bytes(&self) -> ByteOffset {
        ByteOffset::new(self.text.len())
    }

    fn line_count(&self) -> usize {
        self.text.split('\n').count()
    }

    fn line_start(&self, line: Line) -> TextResult<ByteOffset> {
        if line.get() >= self.line_count() {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }
        let mut byte = 0;
        for _ in 0..line.get() {
            byte += self.text[byte..]
                .find('\n')
                .map_or(self.text.len() - byte, |index| index + 1);
        }
        Ok(ByteOffset::new(byte))
    }

    fn byte_to_position(&self, offset: ByteOffset) -> TextResult<Position> {
        let byte = self.validate_offset(offset)?;
        let prefix = &self.text[..byte];
        let line = prefix.bytes().filter(|&b| b == b'\n').count();
        let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
        let column = self.text[line_start..byte].chars().count();
        Ok(Position::new(Line::new(line), LogicalColumn::new(column)))
    }

    fn position_to_byte(&self, position: Position) -> TextResult<ByteOffset> {
        let line = position.line();
        if line.get() >= self.line_count() {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }
        let line_start = self.line_start(line)?.get();
        let line_end = self.text[line_start..]
            .find('\n')
            .map_or(self.text.len(), |index| line_start + index);
        let mut byte = line_start;
        let mut column = position.column().get();
        for (index, _) in self.text[line_start..line_end].char_indices() {
            if column == 0 {
                byte = line_start + index;
                break;
            }
            column -= 1;
        }
        if column > 0 {
            byte = line_end;
        }
        Ok(ByteOffset::new(byte))
    }

    fn is_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<bool> {
        let byte = self.validate_offset(offset)?;
        Ok(byte == self.text.len()
            || self
                .text
                .grapheme_indices(true)
                .any(|(start, _)| start == byte))
    }

    fn previous_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset> {
        let byte = self.validate_offset(offset)?;
        let previous = self.text[..byte]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(start, _)| start);
        Ok(ByteOffset::new(previous))
    }

    fn next_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset> {
        let byte = self.validate_offset(offset)?;
        let next = self.text[byte..]
            .grapheme_indices(true)
            .nth(1)
            .map_or(self.text.len(), |(start, _)| byte + start);
        Ok(ByteOffset::new(next))
    }

    fn char_at_byte(&self, offset: ByteOffset) -> Option<char> {
        let byte = offset.get();
        if byte >= self.text.len() || !self.text.is_char_boundary(byte) {
            return None;
        }
        self.text[byte..].chars().next()
    }

    fn char_to_position(&self, offset: CharOffset) -> TextResult<Position> {
        let byte = self.char_to_byte(offset)?;
        self.byte_to_position(byte)
    }

    fn position_to_char(&self, position: Position) -> TextResult<CharOffset> {
        let byte = self.position_to_byte(position)?;
        self.byte_to_char(byte)
    }

    fn char_at(&self, offset: CharOffset) -> Option<char> {
        self.text.chars().nth(offset.get())
    }

    fn len_chars(&self) -> CharOffset {
        CharOffset::new(self.text.chars().count())
    }

    fn char_to_byte(&self, offset: CharOffset) -> TextResult<ByteOffset> {
        if offset.get() > self.len_chars().get() {
            return Err(CoordinateError::CharOutOfBounds(offset).into());
        }
        let byte = self
            .text
            .char_indices()
            .nth(offset.get())
            .map_or(self.text.len(), |(index, _)| index);
        Ok(ByteOffset::new(byte))
    }

    fn byte_to_char(&self, offset: ByteOffset) -> TextResult<CharOffset> {
        let byte = self.validate_offset(offset)?;
        Ok(CharOffset::new(self.text[..byte].chars().count()))
    }

    fn byte_to_utf16_cu(&self, offset: ByteOffset) -> TextResult<Utf16Offset> {
        let byte = self.validate_offset(offset)?;
        Ok(Utf16Offset::new(self.text[..byte].encode_utf16().count()))
    }

    fn utf16_cu_to_byte(&self, offset: Utf16Offset) -> TextResult<ByteOffset> {
        let mut units = 0;
        for (index, ch) in self.text.char_indices() {
            if units >= offset.get() {
                return Ok(ByteOffset::new(index));
            }
            units += ch.len_utf16();
        }
        if units == offset.get() {
            return Ok(ByteOffset::new(self.text.len()));
        }
        Err(CoordinateError::InvalidUtf16Boundary(offset).into())
    }

    fn line_ending_style(&self) -> LineEndingStyle {
        if self.text.contains("\r\n") {
            LineEndingStyle::Crlf
        } else if self.text.contains('\n') {
            LineEndingStyle::Lf
        } else {
            LineEndingStyle::None
        }
    }
}

/// 上次选区手势的粒度。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Granularity {
    Character,
    Word,
    Block,
    All,
}

impl Granularity {
    fn click_count(self) -> usize {
        match self {
            Self::Character => 1,
            Self::Word => 2,
            Self::Block => 3,
            Self::All => 4,
        }
    }
}

/// 进行中的鼠标选区手势。
#[derive(Clone)]
struct PendingGesture {
    /// 手势锚点（固定端）。
    anchor: usize,
    granularity: Granularity,
    /// 词/块粒度下按下手势时的原始范围，拖拽时按它吸附。
    original: Range<usize>,
}

/// 预览的选择状态与鼠标手势状态机。
///
/// 选择偏移定义在 [`SelectableText`] 上；状态由鼠标事件驱动，
/// 跨帧稳定，只在块树重建（源码变化）时由 `reset` 清空。
pub(crate) struct SelectionModel {
    selection: Option<Range<usize>>,
    /// 最近一次手势的锚点；Shift+点击以它为固定端扩展。
    anchor: usize,
    /// 是否存在可扩展的手势锚点；reset 后失效。
    has_anchor: bool,
    /// 选区活动端；键盘扩展从它出发移动，鼠标手势把它更新到手势头部。
    head: usize,
    /// 键盘扩展序列的固定端；序列内保持不变，鼠标手势或 reset 重新确定。
    extend_tail: Option<usize>,
    pending: Option<PendingGesture>,
    granularity: Granularity,
    /// 上次手势的粒度范围；Shift+点击扩展时与本次范围求并。
    granularity_range: Range<usize>,
}

impl Default for SelectionModel {
    fn default() -> Self {
        Self {
            selection: None,
            anchor: 0,
            has_anchor: false,
            head: 0,
            extend_tail: None,
            pending: None,
            granularity: Granularity::Character,
            granularity_range: 0..0,
        }
    }
}

impl SelectionModel {
    pub(crate) fn range(&self) -> Option<Range<usize>> {
        self.selection.clone()
    }

    pub(crate) fn has_pending_selection(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// 鼠标按下：按点击次数确定粒度并开始手势。
    ///
    /// 单击定位、双击选词、三击选块、四击及以上全选；
    /// `extend`（Shift 按下）时按上次手势粒度提升本次点击次数，
    /// 以最近锚点为固定端向两侧扩展，点击范围覆盖锚点时整段纳入。
    pub(crate) fn begin_selection(
        &mut self,
        selectable: &SelectableText,
        offset: usize,
        click_count: usize,
        extend: bool,
    ) {
        let click_count = if extend {
            click_count.max(self.granularity.click_count())
        } else {
            click_count
        };
        let (range, granularity) = match click_count {
            1 => (offset..offset, Granularity::Character),
            2 => selectable
                .surrounding_word(offset)
                .map(|range| (range, Granularity::Word))
                .unwrap_or((offset..offset, Granularity::Character)),
            3 => selectable
                .block_range(offset)
                .map(|range| (range, Granularity::Block))
                .unwrap_or((offset..offset, Granularity::Character)),
            _ => (0..selectable.len(), Granularity::All),
        };

        if extend && self.has_anchor {
            let tail = self.anchor;
            let mut start = range.start.min(tail);
            let mut end = range.end.max(tail);
            // 与上次手势的粒度范围求并，后续拖拽按并集吸附。
            start = start.min(self.granularity_range.start);
            end = end.max(self.granularity_range.end);
            self.selection = Some(start..end);
            // 活动端取本次范围中远离锚点的一端，后续键盘扩展从它出发。
            self.head = if (range.end as isize - tail as isize).abs()
                >= (range.start as isize - tail as isize).abs()
            {
                range.end
            } else {
                range.start
            };
            self.extend_tail = None;
            self.pending = Some(PendingGesture {
                anchor: tail,
                granularity,
                original: range,
            });
        } else {
            self.granularity = granularity;
            self.granularity_range = range.clone();
            self.anchor = offset;
            self.has_anchor = true;
            self.head = range.end;
            self.extend_tail = None;
            self.selection = Some(range.clone());
            self.pending = Some(PendingGesture {
                anchor: offset,
                granularity,
                original: range,
            });
        }
    }

    /// 鼠标拖动：按按下时的粒度把选区活动端更新到当前命中位置。
    ///
    /// 词/块粒度下按整词/整块边界吸附，避免半词截断。
    pub(crate) fn update_selection(&mut self, selectable: &SelectableText, offset: usize) {
        let Some(gesture) = self.pending.clone() else {
            return;
        };
        let (head, tail) = match gesture.granularity {
            Granularity::Character => (offset, gesture.anchor),
            Granularity::Word => {
                let inside =
                    selectable.is_inside_word(offset) || gesture.original.contains(&offset);
                let head = if inside {
                    selectable.surrounding_word(offset).map_or(offset, |word| {
                        if word.start < gesture.original.start {
                            word.start
                        } else {
                            word.end
                        }
                    })
                } else {
                    offset
                };
                if head <= gesture.original.start {
                    (head, gesture.original.end)
                } else {
                    (head, gesture.original.start)
                }
            }
            Granularity::Block => {
                let head = selectable.block_range(offset).map_or(offset, |block| {
                    if block.start < gesture.original.start {
                        block.start
                    } else {
                        block.end
                    }
                });
                if head <= gesture.original.start {
                    (head, gesture.original.end)
                } else {
                    (head, gesture.original.start)
                }
            }
            Granularity::All => return,
        };
        self.head = head;
        self.extend_tail = None;
        self.selection = Some(head.min(tail)..head.max(tail));
    }

    /// 鼠标松开：结束手势；空选区不保留。
    pub(crate) fn end_selection(&mut self) {
        self.pending = None;
        if self.selection.as_ref().is_some_and(Range::is_empty) {
            self.selection = None;
        }
    }

    pub(crate) fn select_all(&mut self, selectable: &SelectableText) {
        self.pending = None;
        self.granularity = Granularity::All;
        self.anchor = 0;
        self.granularity_range = 0..0;
        self.head = selectable.len();
        self.extend_tail = None;
        self.selection = (!selectable.is_empty()).then(|| 0..selectable.len());
    }

    /// 选区活动端；键盘扩展从它出发。
    pub(crate) fn head(&self) -> usize {
        self.head
    }

    /// 是否存在可显示光标的锚点；reset 后失效。
    pub(crate) fn has_anchor(&self) -> bool {
        self.has_anchor
    }

    /// 键盘扩展：把活动端移动到指定偏移，固定端在整个键盘序列中保持不变。
    ///
    /// 序列的首次扩展从当前选区推导固定端（活动端之外的那端），无选区时从最近锚点出发；
    /// 反向移动越过固定端时选区随之反向。
    pub(crate) fn extend_to(&mut self, offset: usize) {
        let tail = match self.extend_tail {
            Some(tail) => tail,
            None => {
                let tail = match &self.selection {
                    Some(range) if range.start != self.head => range.start,
                    Some(range) if range.end != self.head => range.end,
                    _ => self.anchor,
                };
                self.extend_tail = Some(tail);
                tail
            }
        };
        self.head = offset;
        self.selection = (offset != tail).then(|| tail.min(offset)..tail.max(offset));
    }

    pub(crate) fn selected_text(&self, selectable: &SelectableText) -> Option<String> {
        let range = self.selection.as_ref()?;
        (!range.is_empty()).then(|| selectable.text()[range.clone()].to_owned())
    }
}

/// 行内公式占位在渲染文本与选择文本之间的区间映射（run 局部字节坐标）。
#[derive(Clone)]
pub(crate) struct PlaceholderMap {
    /// 渲染文本中的占位区间（不可断行空格）。
    pub(crate) render: Range<usize>,
    /// 选择文本中的公式源码区间。
    pub(crate) selectable: Range<usize>,
}

/// 选区与本 run 的渲染交集：裁剪后的渲染区间与选区是否终止于本 run。
#[derive(Debug, PartialEq)]
pub(crate) struct RenderSelection {
    pub(crate) range: Range<usize>,
    /// 选区终点落在本 run 内：末行不做行尾延伸。
    pub(crate) ends_here: bool,
}

/// 当前帧的命中几何：渲染逐帧重建，不是位置权威。
#[derive(Default)]
pub(crate) struct SelectionLayout {
    runs: Vec<SelectionRun>,
    /// 本帧待消费的片段区间，与渲染的文本单元一一对应。
    segments: Vec<Range<usize>>,
    next_segment: usize,
}

/// run 渲染文本内的链接区间与目标。
pub(crate) struct RunLink {
    /// 链接在渲染文本中的字节区间。
    pub(crate) range: Range<usize>,
    /// 链接目标（URL、相对路径或文档内锚点）。
    pub(crate) target: String,
}

struct SelectionRun {
    /// 该 run 覆盖的选择坐标区间。
    selectable: Range<usize>,
    /// 渲染文本的字节长度（含公式占位）。
    render_len: usize,
    placeholders: Vec<PlaceholderMap>,
    layout: TextLayout,
    bounds: Bounds<Pixels>,
    links: Vec<RunLink>,
}

impl SelectionLayout {
    /// 开始新的一帧：清空几何并载入本帧的片段区间。
    pub(crate) fn begin_frame(&mut self, segments: &[Range<usize>]) {
        self.runs.clear();
        self.segments = segments.to_vec();
        self.next_segment = 0;
    }

    /// 为下一个渲染文本单元注册命中几何，返回 run 索引。
    ///
    /// 片段区间由块树投影提供，渲染必须按相同顺序消费；
    /// 顺序错位说明渲染与投影的遍历结构已分叉，显式失败。
    pub(crate) fn register_next_run(
        &mut self,
        render_len: usize,
        placeholders: Vec<PlaceholderMap>,
        layout: TextLayout,
    ) -> usize {
        let segment = self
            .segments
            .get(self.next_segment)
            .cloned()
            .expect("渲染与选择投影的片段顺序必须一致");
        self.next_segment += 1;
        let index = self.runs.len();
        self.runs.push(SelectionRun {
            selectable: segment,
            render_len,
            placeholders,
            layout,
            bounds: Bounds::default(),
            links: Vec::new(),
        });
        index
    }

    /// 记录 run 渲染文本内的链接区间；每帧渲染时重新登记。
    pub(crate) fn set_run_links(&mut self, index: usize, links: Vec<RunLink>) {
        if let Some(run) = self.runs.get_mut(index) {
            run.links = links;
        }
    }

    pub(crate) fn set_run_bounds(&mut self, index: usize, bounds: Bounds<Pixels>) {
        if let Some(run) = self.runs.get_mut(index) {
            run.bounds = bounds;
        }
    }

    /// 纵向覆盖指定纵坐标的 run（表格同行多格会有多个）。
    fn covering_runs(&self, y: Pixels) -> Vec<&SelectionRun> {
        self.runs
            .iter()
            .filter(|run| y >= run.bounds.origin.y && y <= run.bounds.bottom())
            .collect()
    }

    /// 在纵向覆盖的 run 中按横向归属选出目标 run：
    /// 区间内直接归属，间隙按中点分界，整行之外钳到首末 run；
    /// 间隙归属的 run 经行内钳制自然落到前 run 行尾或本 run 行首。
    fn pick_run_by_x<'a>(
        &self,
        covered: &[&'a SelectionRun],
        x: Pixels,
    ) -> Option<&'a SelectionRun> {
        let mut previous: Option<&'a SelectionRun> = None;
        for run in covered {
            if x <= run.bounds.right() {
                return Some(if x >= run.bounds.origin.x {
                    run
                } else {
                    match previous {
                        Some(previous) => {
                            let middle = (previous.bounds.right() + run.bounds.origin.x) / 2.;
                            if x < middle { previous } else { run }
                        }
                        None => covered.first()?,
                    }
                });
            }
            previous = Some(run);
        }
        covered.last().copied()
    }

    /// 任意像素位置的最近选择偏移；无可选文本时返回 None。
    ///
    /// 对齐 Editor 的命中语义：内容上下越界钳制到首末片段端点，
    /// 片段间隙按中点分界归属相邻片段，行内由文本布局钳制；
    /// 因此点击块间空白、拖拽跨越间隙都能得到确定的位置。
    /// 同一行可能存在多个横向并列的 run（表格单元格）：
    /// 先收集纵向覆盖的 run，再按横向区间归属，横向间隙同样按中点分界。
    pub(crate) fn index_for_position(&self, position: Point<Pixels>) -> Option<usize> {
        let first = self.runs.first()?;
        let last = self.runs.last()?;
        if position.y < first.bounds.origin.y {
            return Some(first.selectable.start);
        }
        if position.y > last.bounds.bottom() {
            return Some(last.selectable.end);
        }
        let covered = self.covering_runs(position.y);
        if let Some(run) = self.pick_run_by_x(&covered, position.x) {
            let local = match run.layout.index_for_position(position) {
                Ok(index) | Err(index) => index,
            };
            return Some(run.selectable.start + run.map_render_to_selectable(local));
        }
        // 纵向间隙：与本位置上下相邻的 run 按中点分界。
        let below = self
            .runs
            .iter()
            .find(|run| position.y < run.bounds.origin.y);
        match below {
            Some(below) => {
                let above = self
                    .runs
                    .iter()
                    .filter(|run| position.y > run.bounds.bottom())
                    .max_by(|a, b| a.bounds.bottom().cmp(&b.bounds.bottom()));
                match above {
                    Some(above) => {
                        let middle = (above.bounds.bottom() + below.bounds.origin.y) / 2.;
                        Some(if position.y < middle {
                            above.selectable.end
                        } else {
                            below.selectable.start
                        })
                    }
                    None => Some(below.selectable.start),
                }
            }
            None => self
                .runs
                .iter()
                .max_by(|a, b| a.bounds.bottom().cmp(&b.bounds.bottom()))
                .map(|run| run.selectable.end),
        }
    }

    /// position 精确命中的链接目标；链接文本之外的间隙与钳制位置不返回。
    pub(crate) fn link_for_position(&self, position: Point<Pixels>) -> Option<String> {
        let covered = self.covering_runs(position.y);
        let run = self.pick_run_by_x(&covered, position.x)?;
        let local = run.layout.index_for_position(position).ok()?;
        run.links
            .iter()
            .find(|link| link.range.contains(&local))
            .map(|link| link.target.clone())
    }

    /// 选区与本 run 的交集映射回渲染文本区间；无交集返回 None。
    pub(crate) fn render_range_for(
        &self,
        run_index: usize,
        selection: &Range<usize>,
    ) -> Option<RenderSelection> {
        let run = self.runs.get(run_index)?;
        let clipped_start = selection.start.max(run.selectable.start);
        let clipped_end = selection.end.min(run.selectable.end);
        if clipped_start >= clipped_end {
            return None;
        }
        let start = run.map_selectable_to_render(clipped_start - run.selectable.start, true);
        let end = run.map_selectable_to_render(clipped_end - run.selectable.start, false);
        Some(RenderSelection {
            range: start..end,
            ends_here: selection.end <= run.selectable.end,
        })
    }

    /// 光标偏移在本 run 的渲染偏移；段间分隔符偏移归属前一个 run 的末尾。
    ///
    /// 返回 None 表示光标不在本 run（含其尾部间隙）内。
    pub(crate) fn render_offset_for(&self, run_index: usize, offset: usize) -> Option<usize> {
        let run = self.runs.get(run_index)?;
        let next = self.runs.get(run_index + 1);
        let in_run = run.selectable.start <= offset && offset <= run.selectable.end;
        // 段间偏移：本 run 末尾之后、下一 run 开头之前（块间换行、单元格间制表符）。
        let in_gap =
            offset > run.selectable.end && next.is_some_and(|next| offset < next.selectable.start);
        if !in_run && !in_gap {
            return None;
        }
        let local = offset.min(run.selectable.end) - run.selectable.start;
        Some(run.map_selectable_to_render(local, true))
    }

    /// 键盘上下移动：把选择偏移在渲染几何中上下移动一行，返回新的选择偏移。
    ///
    /// 返回 None 表示无可移动的目标（首行向上、行距缝隙内无下一行）
    /// 或当前帧没有该偏移的几何。命中取目标行的垂直中点：
    /// gpui 的行命中按 y 整除行高判行，落在行边界上会回退到不存在的下一行。
    pub(crate) fn vertical_move(&self, offset: usize, up: bool) -> Option<usize> {
        // 偏移归属的 run：段间分隔符偏移用前一个 run 的末尾位置。
        let run = self
            .runs
            .iter()
            .rev()
            .find(|run| run.selectable.start <= offset)?;
        let local = offset.min(run.selectable.end) - run.selectable.start;
        let render_local = run.map_selectable_to_render(local, true);
        let position = run.layout.position_for_index(render_local)?;
        let line_height = run.layout.line_height();
        let target_y = if up {
            position.y - line_height
        } else {
            position.y + line_height
        };
        let hit_y = target_y + line_height / 2.;
        if up {
            // 目标行中点已在内容上方：首行保持不动，不跳到文档开头。
            if hit_y < self.runs.first()?.bounds.origin.y {
                return None;
            }
            // 目标行在本 run 内按行中点命中；跨块时中点落在上方块或间隙，
            // 由全局命中按间隙中点分界。
            return self.index_for_position(point(position.x, hit_y));
        }
        // 本 run 还有下一行：行中点命中。
        let last_line_y = run.layout.position_for_index(run.render_len)?.y;
        if target_y <= last_line_y {
            return self.index_for_position(point(position.x, hit_y));
        }
        if target_y >= run.bounds.bottom() {
            // 超出本 run：全局命中（跨块、间隙分界或钳制到末段终点）。
            return self.index_for_position(point(position.x, hit_y));
        }
        // 行距缝隙内没有下一行：保持不动。
        None
    }
}

impl SelectionRun {
    /// 渲染文本局部偏移 → 选择文本局部偏移。
    ///
    /// 公式占位区域按占位中心分界：前半命中到公式起点，后半命中到公式终点。
    fn map_render_to_selectable(&self, render_offset: usize) -> usize {
        let mut delta: isize = 0;
        for placeholder in &self.placeholders {
            if render_offset < placeholder.render.start {
                break;
            }
            let center = placeholder.render.start + placeholder.render.len() / 2;
            if render_offset <= center {
                return placeholder.selectable.start;
            }
            if render_offset < placeholder.render.end {
                return placeholder.selectable.end;
            }
            delta += placeholder.selectable.len() as isize - placeholder.render.len() as isize;
        }
        (render_offset as isize + delta).clamp(0, self.selectable.len() as isize) as usize
    }

    /// 选择文本局部偏移 → 渲染文本局部偏移。
    ///
    /// 偏移落在公式源码区间端点时按端点语义映射：公式起点对应占位左端、
    /// 公式终点对应占位右端，因此选区在公式前结束不触及占位、
    /// 覆盖公式的选区高亮整个占位区域；落在区间内部时进入侧取左端、离开侧取右端。
    fn map_selectable_to_render(&self, selectable_offset: usize, entering: bool) -> usize {
        let mut delta: isize = 0;
        for placeholder in &self.placeholders {
            if selectable_offset < placeholder.selectable.start {
                break;
            }
            if selectable_offset == placeholder.selectable.start {
                return placeholder.render.start;
            }
            if selectable_offset < placeholder.selectable.end {
                return if entering {
                    placeholder.render.start
                } else {
                    placeholder.render.end
                };
            }
            if selectable_offset == placeholder.selectable.end {
                return placeholder.render.end;
            }
            delta += placeholder.render.len() as isize - placeholder.selectable.len() as isize;
        }
        (selectable_offset as isize + delta).clamp(0, self.render_len as isize) as usize
    }
}

#[cfg(test)]
#[path = "test/selection_tests.rs"]
mod tests;
