//! 字节级 chunk：文本切片 + 字符/tab 位图。
//!
//! 位图让"任意字节边界切分"与"tab 展开/字符坐标换算"变成 O(1) 位运算，无需逐字符扫描：
//! - `chars`：每个 UTF-8 字符的起始字节 bit=1（LSB 对应文本字节 0）；
//! - `tabs`：每个 tab 字节 bit=1。
//!
//! 渲染层按行消费 chunk 流（128 字节对齐）；
//! 跨行的换行位图当前行级渲染不需要，裁掉。
//!
//! 基础文本 chunk 经样式与 tab 变换，产出带样式与 is_tab 标记的渲染 chunk；
//! 渲染端逐 chunk 生成 TextRun 后统一 shape。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::borrow::Cow;
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use gpui::{HighlightStyle, UnderlineStyle, px};
use zcv_language::HighlightSpan;
use zcv_multi_buffer::MultiBufferSnapshot;
use zcv_text::Line;

use super::block_map::{BlockRow, BlockRows, DisplayBlock};
use super::fold_map::{ChunkRenderer, FoldRowSegment, FoldRowSegmentKind, ProjectedLineIndex};
use super::tab_map::advance_display_column;
use super::wrap_map::WrapRowKind;
use super::{DisplayRow, DisplaySnapshot};
use zcv_multi_buffer::ExcerptSnapshot;

/// chunk 文本字节上限。
pub(crate) const CHUNK_SIZE: usize = 128;

/// 单个显示行交给文字 shaping 的最大字节数。
pub(crate) const MAX_RENDERED_LINE_LEN: usize = 1024;

/// 渲染 chunk：文本切片 + 字符/tab 位图 + 样式标记（is_tab/highlight_style）。
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Chunk<'a> {
    pub(crate) text: &'a str,
    pub(crate) chars: u128,
    pub(crate) tabs: u128,
    /// 是否由 tab 展开而来（tab 展开的空格段）。
    pub(crate) is_tab: bool,
    /// 折叠占位符文本（渲染端用占位色绘制）。
    pub(crate) is_placeholder: bool,
    /// 占位符的渲染描述；Some 时显示层把该段替换为元素而不是绘制文本。
    pub(crate) renderer: Option<ChunkRenderer>,
    pub(crate) style: Option<HighlightStyle>,
    /// 背景覆盖层命中色（搜索高亮等；优先于 style 的背景）。
    pub(crate) background: Option<gpui::Rgba>,
    /// 选区标记（下划线渲染）。
    pub(crate) marked: bool,
    /// 局部重命名期间淡化原名称及其引用。
    pub(crate) dimmed: bool,
}

impl<'a> Chunk<'a> {
    /// 从文本构建位图（逐字符扫描，O(字符数)）。
    pub(crate) fn from_text(text: &'a str) -> Self {
        let mut chars = 0u128;
        let mut tabs = 0u128;
        for (index, ch) in text.char_indices() {
            chars |= 1u128 << index;
            if ch == '\t' {
                tabs |= 1u128 << index;
            }
        }
        Self {
            text,
            chars,
            tabs,
            ..Default::default()
        }
    }

    /// 在文本中的字符边界处切分，位图与样式元数据随 chunk 一起变换。
    ///
    /// 本方法不修正调用者给出的坐标：
    /// chunk 流的构造者负责保证边界，变换层只能在 `chars` 位图标记的位置切分。
    pub(crate) fn split_at(self, mid: usize) -> (Self, Self) {
        assert!(
            mid <= self.text.len() && self.text.is_char_boundary(mid),
            "chunk transforms must split at a UTF-8 character boundary"
        );
        let mask = if mid == u128::BITS as usize {
            u128::MAX
        } else {
            (1u128 << mid).wrapping_sub(1)
        };
        let (left_text, right_text) = self.text.split_at(mid);
        let mut left = self.clone();
        left.text = left_text;
        left.chars &= mask;
        left.tabs &= mask;
        let mut right = self;
        right.text = right_text;
        if mid == u128::BITS as usize {
            right.chars = 0;
            right.tabs = 0;
        } else {
            right.chars >>= mid;
            right.tabs >>= mid;
        }
        (left, right)
    }

    /// tab 前的字符数（tab 宽度取模用）。
    pub(crate) fn chars_before_tab(&self, tab_byte: usize) -> usize {
        (self.chars & ((1u128 << tab_byte).wrapping_sub(1))).count_ones() as usize
    }
}

/// 静态空格表（tab 展开的空格段借用它；tab 宽度对齐的跨度 ≤ tab_width）。
const SPACES: &str = "                                                                ";

/// 展开 tab 后的 chunk 流（tabs 位图驱动）。
///
/// 展开与测量（`advance_display_column`）同规则：tab 宽度 = `tab_width - col % tab_width`；
/// 展开前的起始列作为片段内展开的列对齐基准。
/// 输出的 tab 段标记 `is_tab`，文本借用静态空格表。
struct TabChunks<'a, I>
where
    I: Iterator<Item = Chunk<'a>>,
{
    source: I,
    /// 当前 chunk（消费中；None = 取下一个）。
    current: Option<Chunk<'a>>,
    /// 上次 head 段之后待展开的 tab 宽度（tab 与文本段分两次输出）。
    pending_tab: Option<(usize, Chunk<'a>)>,
    tab_width: usize,
    /// 展开后列（tab 对齐基准）。
    column: usize,
}

impl<'a, I> TabChunks<'a, I>
where
    I: Iterator<Item = Chunk<'a>>,
{
    fn from_chunks(source: I, tab_width: usize, start_column: usize) -> Self {
        Self {
            source,
            current: None,
            pending_tab: None,
            tab_width,
            column: start_column,
        }
    }
}

impl<'a, I> Iterator for TabChunks<'a, I>
where
    I: Iterator<Item = Chunk<'a>>,
{
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // 优先输出待展开的 tab（head 段之后）。
        if let Some((width, mut tab)) = self.pending_tab.take() {
            self.column += width;
            tab.text = &SPACES[..width];
            tab.chars = (1u128 << width) - 1;
            tab.tabs = 0;
            tab.is_tab = true;
            return Some(tab);
        }
        let chunk = match self.current.take() {
            Some(chunk) => chunk,
            None => self.source.next()?,
        };
        if chunk.tabs == 0 {
            // 快速路径：无 tab，整段透传（列按字符数推进）。
            let chars = chunk.chars.count_ones() as usize;
            self.column += chars;
            self.current = None;
            if chunk.text.is_empty() {
                // 段尾空残段（tab 后无文本）不输出。
                return self.next();
            }
            return Some(Chunk {
                is_tab: false,
                ..chunk
            });
        }
        // 一般路径：切出到下一个 tab 前的文本段；tab 段在下次 next 输出。
        let tab_byte = chunk.tabs.trailing_zeros() as usize;
        let before = chunk.chars_before_tab(tab_byte);
        let (head, rest) = chunk.split_at(tab_byte);
        let (tab, after_tab) = rest.split_at(1);
        self.current = Some(after_tab);
        self.column += before;
        if !head.text.is_empty() {
            // tab 宽度在输出 head 后计算（列已推进）。
            let width = self.tab_width - self.column % self.tab_width;
            self.pending_tab = Some((width, tab));
            return Some(Chunk {
                is_tab: false,
                ..head
            });
        }
        // tab 在段首：直接展开（宽度按当前列对齐）。
        let width = self.tab_width - self.column % self.tab_width;
        self.column += width;
        let mut tab = tab;
        tab.text = &SPACES[..width];
        tab.chars = (1u128 << width) - 1;
        tab.tabs = 0;
        tab.is_tab = true;
        Some(tab)
    }
}

/// 行的样式输入（语法高亮 + 搜索背景层 + 选区标记）。
///
/// 独立于语法前景色的背景覆盖层：
/// 搜索匹配等只改背景、保留语法前景色的场景走这一层，不经过 spans 的 style 替换。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct HighlightStyles<'a> {
    pub(crate) spans: &'a [HighlightSpan],
    pub(crate) styles: &'a [HighlightStyle],
    /// 背景覆盖层：命中区间优先于语法 style 的背景色。
    pub(crate) backgrounds: &'a [(Range<usize>, gpui::Rgba)],
    pub(crate) marked: &'a [MultiBufferRange],
    /// 局部重命名期间需要淡化的文本范围。
    pub(crate) dimmed: &'a [Range<usize>],
}

impl<'a> HighlightStyles<'a> {
    /// 将全视口样式事件裁剪为当前源行窗口；行内扫描不再从视口首个事件重新查找。
    fn for_range(&self, range: Range<usize>) -> Self {
        let span_start = self
            .spans
            .partition_point(|span| span.range.end <= range.start);
        let span_end = self.spans[span_start..]
            .partition_point(|span| span.range.start < range.end)
            + span_start;
        let background_start = self
            .backgrounds
            .partition_point(|(item, _)| item.end <= range.start);
        let background_end = self.backgrounds[background_start..]
            .partition_point(|(item, _)| item.start < range.end)
            + background_start;
        let marked_start = self
            .marked
            .partition_point(|item| item.end().get() <= range.start);
        let marked_end = self.marked[marked_start..]
            .partition_point(|item| item.start().get() < range.end)
            + marked_start;
        let dimmed_start = self.dimmed.partition_point(|item| item.end <= range.start);
        let dimmed_end = self.dimmed[dimmed_start..].partition_point(|item| item.start < range.end)
            + dimmed_start;
        Self {
            spans: &self.spans[span_start..span_end],
            styles: self.styles,
            backgrounds: &self.backgrounds[background_start..background_end],
            marked: &self.marked[marked_start..marked_end],
            dimmed: &self.dimmed[dimmed_start..dimmed_end],
        }
    }
}

#[derive(Clone)]
pub(crate) enum ChunkText<'a> {
    Borrowed(&'a str),
    Virtual {
        snapshot: &'a MultiBufferSnapshot,
        range: Range<MultiBufferOffset>,
    },
}

impl ChunkText<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Borrowed(text) => text.len(),
            Self::Virtual { range, .. } => range.end.get() - range.start.get(),
        }
    }
}

pub(crate) struct ChunkSource<'a> {
    pub(crate) text: ChunkText<'a>,
    /// 该显示行的投影文本字节长度；折叠合并行取各段合并范围末端。
    pub(crate) projected_len: usize,
    pub(crate) global_byte_start: usize,
    pub(crate) segments: Option<&'a [FoldRowSegment]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ChunkStyle {
    style: Option<HighlightStyle>,
    /// 背景覆盖层命中色（搜索高亮等；优先于 style 的背景）。
    background: Option<gpui::Rgba>,
    marked: bool,
    dimmed: bool,
}

/// 样式 chunk 游标：输入只能是 `SourceTextChunks` 产生的安全 chunk，输出也只能在输入 chunk 的字符位图边界处分段。
/// 高亮、选区与背景层只决定段的元数据，不直接作为 `str` 的切片下标。
pub(super) struct StyledChunks<'a, 'b> {
    source: SourceTextChunks<'a>,
    current: Option<Chunk<'a>>,
    /// 当前 chunk 在所属投影行内的字节偏移；折叠合并行按各段自身坐标。
    projected_offset: usize,
    global_byte_start: usize,
    text_len: usize,
    /// 本段文本起点在所属投影行内的偏移。
    projected_base: usize,
    styles: HighlightStyles<'b>,
    fragment_range: Range<usize>,
    style_index: usize,
    background_index: usize,
    marked_index: usize,
    dimmed_index: usize,
}

impl<'a, 'b> StyledChunks<'a, 'b> {
    pub(super) fn new(
        text: ChunkText<'a>,
        global_byte_start: usize,
        projected_base: usize,
        styles: HighlightStyles<'b>,
        fragment_range: Range<usize>,
    ) -> Self {
        let text_len = text.len();
        Self {
            source: SourceTextChunks::new(text),
            current: None,
            projected_offset: projected_base,
            global_byte_start,
            text_len,
            projected_base,
            styles,
            fragment_range,
            style_index: 0,
            background_index: 0,
            marked_index: 0,
            dimmed_index: 0,
        }
    }

    /// 投影坐标 → 段内字节偏移；本层直接在源文本坐标上工作。
    fn local(&self, projected: usize) -> usize {
        projected.saturating_sub(self.projected_base)
    }

    fn chunk_style(&mut self, start: usize, end: usize) -> Option<ChunkStyle> {
        if start < self.fragment_range.start || end > self.fragment_range.end {
            return None;
        }
        let original_range = self.local(start)..self.local(end);
        while self
            .styles
            .spans
            .get(self.style_index)
            .is_some_and(|span| span.range.end <= self.global_byte_start + original_range.start)
        {
            self.style_index += 1;
        }
        let style = self.styles.spans.get(self.style_index).and_then(|span| {
            let span_start = span
                .range
                .start
                .saturating_sub(self.global_byte_start)
                .min(self.text_len);
            let span_end = span
                .range
                .end
                .saturating_sub(self.global_byte_start)
                .min(self.text_len);
            (span_start < original_range.end && span_end > original_range.start)
                .then(|| self.styles.styles.get(span.capture as usize).copied())
                .flatten()
        });
        while self
            .styles
            .backgrounds
            .get(self.background_index)
            .is_some_and(|(range, _)| range.end <= self.global_byte_start + original_range.start)
        {
            self.background_index += 1;
        }
        while self
            .styles
            .marked
            .get(self.marked_index)
            .is_some_and(|range| range.end().get() <= self.global_byte_start + original_range.start)
        {
            self.marked_index += 1;
        }
        while self
            .styles
            .dimmed
            .get(self.dimmed_index)
            .is_some_and(|range| range.end <= self.global_byte_start + original_range.start)
        {
            self.dimmed_index += 1;
        }
        let marked = self
            .styles
            .marked
            .get(self.marked_index)
            .is_some_and(|range| {
                let range_start = range
                    .start()
                    .get()
                    .saturating_sub(self.global_byte_start)
                    .min(self.text_len);
                let range_end = range
                    .end()
                    .get()
                    .saturating_sub(self.global_byte_start)
                    .min(self.text_len);
                range_start < original_range.end && range_end > original_range.start
            });
        let dimmed = self
            .styles
            .dimmed
            .get(self.dimmed_index)
            .is_some_and(|range| {
                let range_start = range
                    .start
                    .saturating_sub(self.global_byte_start)
                    .min(self.text_len);
                let range_end = range
                    .end
                    .saturating_sub(self.global_byte_start)
                    .min(self.text_len);
                range_start < original_range.end && range_end > original_range.start
            });
        // 背景覆盖层（搜索高亮）：仅当段完全位于命中区间内才着色。
        // 段与区间部分相交时返回 None，使样式切分扫描在区间边界处切分出精确的子段，避免整段着色吞掉区间外的相邻字符（如紧邻的引号）。
        let background =
            self.styles
                .backgrounds
                .get(self.background_index)
                .and_then(|(range, color)| {
                    let start = range
                        .start
                        .saturating_sub(self.global_byte_start)
                        .min(self.text_len);
                    let end = range
                        .end
                        .saturating_sub(self.global_byte_start)
                        .min(self.text_len);
                    (start <= original_range.start && original_range.end <= end).then_some(*color)
                });
        Some(ChunkStyle {
            style,
            background,
            marked,
            dimmed,
        })
    }
}

/// 源文本 chunk 游标：只遍历 `ChunkText` 指向的源文本切片，128 字节对齐。
#[derive(Clone)]
struct SourceTextChunks<'a> {
    text: ChunkText<'a>,
    offset: usize,
}

impl<'a> SourceTextChunks<'a> {
    fn new(text: ChunkText<'a>) -> Self {
        Self { text, offset: 0 }
    }

    fn next_limited(&mut self, limit: usize) -> Option<Chunk<'a>> {
        if self.offset >= self.text.len() || limit == 0 {
            return None;
        }
        match &self.text {
            ChunkText::Borrowed(text) => {
                let mut end = (self.offset + limit).min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                let chunk = Chunk::from_text(&text[self.offset..end]);
                self.offset = end;
                Some(chunk)
            }
            ChunkText::Virtual { snapshot, range } => {
                let absolute = MultiBufferOffset::new(range.start.get() + self.offset);
                let chunk = snapshot.bytes_in_range(absolute..range.end).next()?;
                let available = (range.end.get() - absolute.get()).min(chunk.text.len());
                let mut end = available.min(limit);
                while !chunk.text.is_char_boundary(end) {
                    end -= 1;
                }
                let result = Chunk::from_text(&chunk.text[..end]);
                self.offset += end;
                Some(result)
            }
        }
    }
}

impl<'a> Iterator for SourceTextChunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_limited(CHUNK_SIZE)
    }
}

impl<'a> Iterator for StyledChunks<'a, '_> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let chunk = self.current.take().or_else(|| self.source.next())?;
            let mut characters = chunk.text.char_indices().peekable();
            characters.next()?;
            let first_end = characters
                .peek()
                .map_or(chunk.text.len(), |(offset, _)| *offset);
            let chunk_style =
                self.chunk_style(self.projected_offset, self.projected_offset + first_end);
            let split = characters.find_map(|(start, character)| {
                let end = start + character.len_utf8();
                (self.chunk_style(self.projected_offset + start, self.projected_offset + end)
                    != chunk_style)
                    .then_some(start)
            });
            let (mut head, tail) = if let Some(split) = split {
                let (head, tail) = chunk.split_at(split);
                (head, Some(tail))
            } else {
                (chunk, None)
            };
            self.projected_offset += head.text.len();
            self.current = tail;
            let Some(chunk_style) = chunk_style else {
                continue;
            };
            head.style = chunk_style.style;
            head.background = chunk_style.background;
            head.marked = chunk_style.marked;
            head.dimmed = chunk_style.dimmed;
            return Some(head);
        }
    }
}

fn prefix_metrics(text: ChunkText<'_>, projected_end: usize) -> (usize, usize) {
    let mut remaining = projected_end;
    let mut chars = 0;
    let mut utf16 = 0;
    let mut chunks = SourceTextChunks::new(text);
    while remaining > 0 {
        let Some(chunk) = chunks.next() else {
            break;
        };
        let mut end = remaining.min(chunk.text.len());
        while end > 0 && !chunk.text.is_char_boundary(end) {
            end -= 1;
        }
        let prefix = &chunk.text[..end];
        chars += prefix.chars().count();
        utf16 += prefix.chars().map(char::len_utf16).sum::<usize>();
        remaining = remaining.saturating_sub(end);
        if end < chunk.text.len() {
            break;
        }
    }
    (chars, utf16)
}

/// Fold 层：把 anchor、占位符和 close 尾段合并成连续 chunk。
///
/// 合并行内的高亮坐标域是断开的（anchor 行与 close 行是两个字节窗口），不能按单行窗口裁剪 spans，因此逐段调用行级合成：
/// 每段携带自己的行内提示（偏移相对段起点）与全局字节基准，占位符段单独产出。
pub(crate) struct FoldChunks<'a, 'b> {
    segments: &'a [FoldRowSegment],
    buffer: &'a MultiBufferSnapshot,
    styles: HighlightStyles<'b>,
    fragment_range: Range<usize>,
    segment_index: usize,
    current: Option<(StyledChunks<'a, 'b>, bool, Option<ChunkRenderer>)>,
}

impl<'a, 'b> FoldChunks<'a, 'b> {
    pub(super) fn new(
        segments: &'a [FoldRowSegment],
        buffer: &'a MultiBufferSnapshot,
        styles: HighlightStyles<'b>,
        fragment_range: Range<usize>,
    ) -> Self {
        Self {
            segments,
            buffer,
            styles,
            fragment_range,
            segment_index: 0,
            current: None,
        }
    }
}

impl<'a, 'b> Iterator for FoldChunks<'a, 'b> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((chunks, is_placeholder, renderer)) = self.current.as_mut() {
                if let Some(mut chunk) = chunks.next() {
                    chunk.is_placeholder = *is_placeholder;
                    chunk.renderer = renderer.clone();
                    return Some(chunk);
                }
                self.current = None;
            }

            let segment = self.segments.get(self.segment_index)?;
            self.segment_index += 1;
            let clipped_start = self
                .fragment_range
                .start
                .max(segment.merged_range.start)
                .min(segment.merged_range.end);
            let clipped_end = self
                .fragment_range
                .end
                .max(segment.merged_range.start)
                .min(segment.merged_range.end);
            if clipped_start >= clipped_end {
                continue;
            }

            let (chunks, is_placeholder, renderer) = match &segment.kind {
                FoldRowSegmentKind::Placeholder { text, renderer } => {
                    // 占位符段始终携带渲染描述：被水平窗口部分覆盖时仍生成行内元素，
                    // 可见性由渲染层的 content mask 裁剪，而不是按视口退化为文本。
                    (
                        StyledChunks::new(
                            ChunkText::Borrowed(text.as_ref()),
                            0,
                            0,
                            HighlightStyles::default(),
                            clipped_start - segment.merged_range.start
                                ..clipped_end - segment.merged_range.start,
                        ),
                        true,
                        Some(renderer.clone()),
                    )
                }
                FoldRowSegmentKind::Text {
                    stream_line,
                    projected_range,
                } => {
                    let projected_start =
                        projected_range.start + clipped_start - segment.merged_range.start;
                    let projected_end =
                        projected_range.start + clipped_end - segment.merged_range.start;
                    let line_range = self
                        .buffer
                        .line_byte_range(*stream_line)
                        .expect("折叠文本段必须位于当前快照内");
                    (
                        StyledChunks::new(
                            ChunkText::Virtual {
                                snapshot: self.buffer,
                                range: MultiBufferOffset::new(
                                    line_range.start.get() + projected_start,
                                )
                                    ..MultiBufferOffset::new(
                                        line_range.start.get() + projected_end,
                                    ),
                            },
                            line_range.start.get(),
                            projected_start,
                            self.styles,
                            projected_start..projected_end,
                        ),
                        false,
                        None,
                    )
                }
            };
            self.current = Some((chunks, is_placeholder, renderer));
        }
    }
}

fn fold_prefix_metrics(
    segments: &[FoldRowSegment],
    buffer: &MultiBufferSnapshot,
    projected_end: usize,
) -> (usize, usize) {
    let mut chars = 0;
    let mut utf16 = 0;
    let chunks = FoldChunks::new(
        segments,
        buffer,
        HighlightStyles::default(),
        0..projected_end,
    );
    for chunk in chunks {
        chars += chunk.text.chars().count();
        utf16 += chunk.text.chars().map(char::len_utf16).sum::<usize>();
    }
    (chars, utf16)
}

/// Wrap 层：把一个显示片段裁剪到可见窗口，并把 Fold 合并行产出的 chunk 交给 tab 展开。
pub(crate) struct WrapChunks<'a, 'b> {
    chunks: WrapChunkSource<'a, 'b>,
    utf16_start: usize,
    remaining: usize,
}

enum WrapChunkSource<'a, 'b> {
    Fold(TabChunks<'a, FoldChunks<'a, 'b>>),
    Plain(TabChunks<'a, StyledChunks<'a, 'b>>),
}

impl<'a, 'b> WrapChunks<'a, 'b> {
    pub(crate) fn new(
        source: ChunkSource<'a>,
        tab_width: usize,
        styles: HighlightStyles<'b>,
        fragment_range: Range<usize>,
        max_rendered_len: usize,
    ) -> Self {
        let fragment_start = fragment_range.start.min(source.projected_len);
        let fragment_end = fragment_range.end.min(source.projected_len);
        let fold_buffer = match &source.text {
            ChunkText::Virtual { snapshot, .. } => Some(*snapshot),
            ChunkText::Borrowed(_) => None,
        };
        let (prefix_chars, prefix_utf16) = if let Some(segments) = source.segments {
            fold_prefix_metrics(
                segments,
                fold_buffer.expect("折叠投影只能作用于 MultiBuffer 文本"),
                fragment_start,
            )
        } else {
            prefix_metrics(source.text.clone(), fragment_start)
        };
        let chunks = if let Some(segments) = source.segments {
            WrapChunkSource::Fold(TabChunks::from_chunks(
                FoldChunks::new(
                    segments,
                    fold_buffer.expect("折叠投影只能作用于 MultiBuffer 文本"),
                    styles,
                    fragment_start..fragment_end,
                ),
                tab_width,
                prefix_chars,
            ))
        } else {
            WrapChunkSource::Plain(TabChunks::from_chunks(
                StyledChunks::new(
                    source.text,
                    source.global_byte_start,
                    0,
                    styles,
                    fragment_start..fragment_end,
                ),
                tab_width,
                prefix_chars,
            ))
        };
        Self {
            chunks,
            utf16_start: prefix_utf16,
            remaining: max_rendered_len,
        }
    }

    pub(crate) fn utf16_start(&self) -> usize {
        self.utf16_start
    }
}

impl<'a, 'b> Iterator for WrapChunks<'a, 'b> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let chunk = match &mut self.chunks {
            WrapChunkSource::Fold(chunks) => chunks.next(),
            WrapChunkSource::Plain(chunks) => chunks.next(),
        }?;
        if chunk.text.len() <= self.remaining {
            self.remaining -= chunk.text.len();
            return Some(chunk);
        }
        let mut end = self.remaining;
        while !chunk.text.is_char_boundary(end) {
            end -= 1;
        }
        self.remaining = 0;
        (end > 0).then(|| chunk.split_at(end).0)
    }
}

/// 未展开 tab 的投影 chunk 流：水平窗口的列→字节换算不再物化整行文本。
enum ProjectedChunkSource<'a, 'b> {
    Fold(FoldChunks<'a, 'b>),
    Plain(StyledChunks<'a, 'b>),
}

impl<'a> Iterator for ProjectedChunkSource<'a, '_> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Fold(chunks) => chunks.next(),
            Self::Plain(chunks) => chunks.next(),
        }
    }
}

fn make_projected_chunks<'a, 'b>(
    source: &ChunkSource<'a>,
    styles: HighlightStyles<'b>,
    fragment_range: Range<usize>,
) -> ProjectedChunkSource<'a, 'b> {
    if let Some(segments) = source.segments {
        let buffer = match &source.text {
            ChunkText::Virtual { snapshot, .. } => *snapshot,
            ChunkText::Borrowed(_) => unreachable!("折叠投影只能作用于 MultiBuffer 文本"),
        };
        ProjectedChunkSource::Fold(FoldChunks::new(segments, buffer, styles, fragment_range))
    } else {
        ProjectedChunkSource::Plain(StyledChunks::new(
            source.text.clone(),
            source.global_byte_start,
            0,
            styles,
            fragment_range,
        ))
    }
}

/// 在未展开 tab 的投影文本里，按显示列定位字节偏移，返回 `(字节, 实际列)`。
///
/// 与 `byte_for_display_column` 的 grapheme / tab 规则一致。
fn projected_byte_for_column(
    chunks: ProjectedChunkSource<'_, '_>,
    tab_width: usize,
    target: usize,
) -> (usize, usize) {
    if target == 0 {
        return (0, 0);
    }
    let mut display = 0usize;
    let mut byte = 0usize;
    for chunk in chunks {
        for grapheme in chunk.text.graphemes(true) {
            let next_display = advance_display_column(display, grapheme, tab_width);
            let next_byte = byte + grapheme.len();
            if target == display {
                return (byte, display);
            }
            if target == next_display {
                return (next_byte, next_display);
            }
            if target > display && target < next_display {
                return if target - display <= next_display - target {
                    (byte, display)
                } else {
                    (next_byte, next_display)
                };
            }
            display = next_display;
            byte = next_byte;
        }
    }
    (byte, display)
}

/// 计算水平窗口在投影文本字节空间的裁剪范围、窗口起点列与窗口前的投影文本。
fn projected_window_metrics(
    source: &ChunkSource<'_>,
    tab_width: usize,
    window: (usize, usize),
) -> (Range<usize>, usize, String) {
    let (start_byte, start_column) = projected_byte_for_column(
        make_projected_chunks(source, HighlightStyles::default(), 0..source.projected_len),
        tab_width,
        window.0,
    );
    let (end_byte, _) = projected_byte_for_column(
        make_projected_chunks(source, HighlightStyles::default(), 0..source.projected_len),
        tab_width,
        window.1,
    );
    let mut prefix = String::new();
    let mut byte = 0usize;
    'outer: for chunk in
        make_projected_chunks(source, HighlightStyles::default(), 0..source.projected_len)
    {
        for grapheme in chunk.text.graphemes(true) {
            if byte >= start_byte {
                break 'outer;
            }
            let next = byte + grapheme.len();
            if next <= start_byte {
                prefix.push_str(grapheme);
            }
            byte = next;
        }
    }
    (start_byte.min(end_byte)..end_byte, start_column, prefix)
}

/// 已进入最终塑形文本的真实空白字符位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedWhitespace {
    /// 在 `display_text` 中的 UTF-8 字节范围。
    pub(crate) byte_range: Range<usize>,
    /// 在完整显示行中的字符列，用于判断是否落入选区。
    pub(crate) display_column: usize,
}

pub(crate) fn chunk_to_run(chunk: &Chunk<'_>, base: gpui::TextRun) -> gpui::TextRun {
    let mut run = gpui::TextRun {
        len: chunk.text.len(),
        ..base
    };
    if let Some(style) = chunk.style {
        if let Some(color) = style.color {
            run.color = color;
        }
        if let Some(weight) = style.font_weight {
            run.font.weight = weight;
        }
        if let Some(font_style) = style.font_style {
            run.font.style = font_style;
        }
        run.background_color = style.background_color;
        run.underline = style.underline;
        run.strikethrough = style.strikethrough;
    }
    if let Some(background) = chunk.background {
        run.background_color = Some(background.into());
    }
    if chunk.marked {
        run.underline = Some(UnderlineStyle {
            color: Some(run.color),
            thickness: px(1.),
            wavy: false,
        });
    }
    if chunk.dimmed {
        run.color.a *= 0.4;
    }
    run
}

/// 一行文本在连续显示流中的借用视图。
///
/// `chunks` 只在回调内有效。
/// 这样 Block/Fold/Wrap 游标能够直接借用快照中的文本，不需要为了跨 `next` 调用保存而复制成 `String` 或 `Vec`。
/// 文本 chunk 只包含该显示行的内容，绝不携带 `\r` 或 `\n` 行终止符；
/// 行终止符仍由下层组合文本保有，用于坐标与行数计算，不能进入 `shape_line` 的单行输入。
pub(crate) struct DisplayTextRow<'a> {
    pub(crate) row: DisplayRow,
    pub(crate) excerpt: Option<&'a ExcerptSnapshot>,
    pub(crate) source_line: usize,
    pub(crate) fragment_index: usize,
    pub(crate) indent: usize,
    pub(crate) utf16_start: usize,
    pub(crate) window_start_column: usize,
    /// 水平窗口之前的投影文本。仅在窗口化时携带，布局层用同一字体测得
    /// 实际像素前缀宽度，不能把 display column 乘拉丁字宽。
    pub(crate) window_prefix: Cow<'a, str>,
}

/// 连续显示行事件。文本 chunk 只能在本次回调中被消费，避免在流中保存自引用状态。
/// `Text` 的 chunk 均是无行终止符的单行内容，布局层可以直接交给单行 shaping。
pub(crate) enum DisplayRowEvent<'a, 'b> {
    Block {
        row: DisplayRow,
        height: usize,
        block: &'a DisplayBlock,
    },
    Text {
        row: DisplayTextRow<'a>,
        chunks: &'a mut WrapChunks<'a, 'b>,
    },
}

/// Block/Fold/Wrap/样式的连续显示行流。
pub(crate) struct BlockChunks<'a, 'b> {
    snapshot: &'a DisplaySnapshot,
    rows: BlockRows<'a>,
    styles: HighlightStyles<'b>,
    tab_width: usize,
    window_columns: Option<(usize, usize)>,
}

impl<'a, 'b> BlockChunks<'a, 'b> {
    pub(crate) fn new(
        snapshot: &'a DisplaySnapshot,
        display_rows: Range<DisplayRow>,
        styles: HighlightStyles<'b>,
        window_columns: Option<(usize, usize)>,
    ) -> Self {
        let line_count = display_rows
            .end
            .get()
            .saturating_sub(display_rows.start.get());
        Self {
            snapshot,
            rows: snapshot.rows(display_rows.start, line_count),
            styles,
            tab_width: snapshot.tab_width().get(),
            window_columns,
        }
    }

    fn with_text_row(
        &self,
        row: BlockRow,
        on_row: &mut impl for<'row> FnMut(DisplayRowEvent<'row, 'b>),
    ) {
        let WrapRowKind::Text {
            byte_range,
            global_byte_start,
            source,
            fragment_index,
            indent,
            projected_line,
        } = row.kind();
        let fold = self.snapshot.wrap_snapshot().tab_snapshot().fold_snapshot();
        let projected = ProjectedLineIndex::new(*projected_line);
        let segments = fold.fold_row_segments(projected);
        let buffer = fold.buffer_snapshot();
        let stream_line = *source;
        let mut range = byte_range.clone();
        let mut window_start_column = 0;
        let mut window_prefix: Cow<'_, str> = Cow::Borrowed("");
        let Some(content_range) = buffer.line_content_byte_range(stream_line) else {
            return;
        };
        let content_len = content_range.end.get() - content_range.start.get();
        let projected_len = if let Some(segments) = segments.as_ref() {
            segments
                .last()
                .expect("折叠合并行必须至少包含一个段")
                .merged_range
                .end
        } else {
            content_len
        };
        let source = ChunkSource {
            text: ChunkText::Virtual {
                snapshot: buffer,
                range: content_range,
            },
            projected_len,
            global_byte_start: *global_byte_start,
            segments: segments.as_deref(),
        };
        if let Some(window) = self.window_columns {
            // 水平窗口的输入是显示列而非字节；沿未展开 tab 的 Fold chunk 游标按显示列累计，
            // 不物化整行投影文本。
            let (window_range, start_column, prefix) =
                projected_window_metrics(&source, self.tab_width, window);
            range.start = range.start.max(window_range.start);
            range.end = range.end.min(window_range.end);
            if range.start > byte_range.start {
                window_start_column = start_column;
                window_prefix = Cow::Owned(prefix);
            }
        }
        let budget = MAX_RENDERED_LINE_LEN.saturating_sub(*indent);
        let line_styles = self
            .styles
            .for_range(*global_byte_start..*global_byte_start + content_len);
        let mut chunks = WrapChunks::new(source, self.tab_width, line_styles, range, budget);
        on_row(DisplayRowEvent::Text {
            row: DisplayTextRow {
                row: row.index(),
                excerpt: row.excerpt(),
                source_line: stream_line.get(),
                fragment_index: *fragment_index,
                indent: *indent,
                utf16_start: chunks.utf16_start(),
                window_start_column,
                window_prefix,
            },
            chunks: &mut chunks,
        });
    }

    /// 从起点开始连续消费虚拟块和文本行。
    ///
    /// 文本回调直接取得 `WrapChunks`，其生命周期严格限制在本行；
    /// 渲染可在这里构建最终 shaping 输入，但显示投影层不再产生中间的所有权 chunk 容器。
    pub(crate) fn for_each_row(
        &mut self,
        mut on_row: impl for<'row> FnMut(DisplayRowEvent<'row, 'b>),
    ) {
        while let Some(row) = self.rows.next() {
            if let Some(block) = row.block() {
                on_row(DisplayRowEvent::Block {
                    row: row.index(),
                    height: row.height(),
                    block,
                });
            } else {
                self.with_text_row(row, &mut on_row);
            }
        }
    }

    pub(crate) fn source_line_ranges(self) -> Vec<Range<Line>> {
        self.rows.source_line_ranges()
    }
}

#[cfg(test)]
#[path = "test/chunk_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "test/backgrounds_layer_tests.rs"]
mod backgrounds_layer_tests;
