//! 字节级 chunk：文本切片 + 字符/tab 位图。
//!
//! 位图让"任意字节边界切分"与"tab 展开/字符坐标换算"变成 O(1) 位运算，无需逐字符扫描：
//! - `chars`：每个 UTF-8 字符的起始字节 bit=1（LSB 对应文本字节 0）；
//! - `tabs`：每个 tab 字节 bit=1。
//!
//! 渲染层按行消费 chunk 流（128 字节对齐）；
//! 跨行的换行位图当前行级渲染不需要，裁掉。
//!
//! 基础文本 chunk 经 inlay、样式与 tab 变换，产出带样式与 is_tab/is_inlay 标记的渲染 chunk；
//! 渲染端逐 chunk 生成 TextRun 后统一 shape。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::{ops::Range, sync::Arc};

use gpui::{HighlightStyle, UnderlineStyle, px};
use zcv_language::HighlightSpan;
use zcv_multi_buffer::MultiBufferSnapshot;
use zcv_text::Line;

use super::block_map::{BlockRow, BlockRows, DisplayBlock};
use super::fold_map::{FOLD_PLACEHOLDER, FoldRowSegment, FoldRowSegmentKind, ProjectedLineIndex};
use super::inlay_map::InlaySnapshot;
use super::tab_map::{byte_for_display_column, display_column_for_byte};
use super::wrap_map::WrapRowKind;
use super::{DisplayRow, DisplaySnapshot};
use zcv_multi_buffer::ExcerptSnapshot;

/// chunk 文本字节上限。
pub(crate) const CHUNK_SIZE: usize = 128;

/// 单个显示行交给文字 shaping 的最大字节数。
pub(crate) const MAX_RENDERED_LINE_LEN: usize = 1024;

/// 行内提示（inlay）的显示信息：注入在投影文本中的一段文本。
///
/// 锚定字符之后的原始行内字节偏移，与注入后（含此前所有注入文本）的投影偏移；
/// 渲染合成与偏移换算共用同一信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlayInfo {
    pub(crate) anchor: usize,
    pub(crate) projected: usize,
    pub(crate) text: Arc<str>,
}

/// 渲染 chunk：文本切片 + 字符/tab 位图 + 样式标记（is_tab/is_inlay/highlight_style）。
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Chunk<'a> {
    pub(crate) text: &'a str,
    pub(crate) chars: u128,
    pub(crate) tabs: u128,
    /// 是否由 tab 展开而来（tab 展开的空格段）。
    pub(crate) is_tab: bool,
    /// 行内提示（inlay）文本（斜体 + 半透明渲染）。
    pub(crate) is_inlay: bool,
    /// 折叠占位符文本（渲染端用占位色绘制）。
    pub(crate) is_placeholder: bool,
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
    /// 注入 inlay（或折叠投影）后的字节长度。
    /// 普通行的 `text` 是源文本，其自身长度不足以表示投影长度。
    pub(crate) projected_len: usize,
    pub(crate) global_byte_start: usize,
    pub(crate) stream_line: Line,
    pub(crate) segments: Option<&'a [FoldRowSegment]>,
    pub(crate) inlay: &'a InlaySnapshot,
    /// 普通行直接借用源文本并在流中注入 inlay；
    /// 折叠行已经提供投影段，不能再次注入，否则虚拟文本会重复。
    pub(crate) inject_inlays: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ChunkStyle {
    is_inlay: bool,
    style: Option<HighlightStyle>,
    /// 背景覆盖层命中色（搜索高亮等；优先于 style 的背景）。
    background: Option<gpui::Rgba>,
    marked: bool,
    dimmed: bool,
}

/// 样式变换：输入只能是 `TextChunks` 产生的安全 chunk，输出也只能在输入 chunk 的字符位图边界处分段。
/// 高亮、选区和 inlay 坐标只决定段的元数据，不直接作为 `str` 的切片下标。
/// inlay 游标在投影与原始坐标中的基准；折叠合并行的两者不同。
#[derive(Clone, Copy)]
pub(super) struct ChunkBase {
    projected: usize,
    original: usize,
}

impl ChunkBase {
    pub(super) const ZERO: Self = Self {
        projected: 0,
        original: 0,
    };

    pub(super) fn new(projected: usize, original: usize) -> Self {
        Self {
            projected,
            original,
        }
    }
}

pub(super) struct InlayChunks<'a, 'b> {
    source: InlayTextChunks<'a>,
    current: Option<Chunk<'a>>,
    projected_offset: usize,
    global_byte_start: usize,
    original_len: usize,
    inlays: &'a [InlayInfo],
    projected_base: usize,
    original_base: usize,
    styles: HighlightStyles<'b>,
    fragment_range: Range<usize>,
    style_index: usize,
    background_index: usize,
    marked_index: usize,
    dimmed_index: usize,
}

impl<'a, 'b> InlayChunks<'a, 'b> {
    pub(super) fn new(
        text: ChunkText<'a>,
        global_byte_start: usize,
        inlays: &'a [InlayInfo],
        base: ChunkBase,
        styles: HighlightStyles<'b>,
        fragment_range: Range<usize>,
        inject_inlays: bool,
    ) -> Self {
        Self {
            source: InlayTextChunks::new(text.clone(), inlays, base.original, inject_inlays),
            current: None,
            projected_offset: base.projected,
            global_byte_start,
            // `text` 始终是源文本切片；游标前进时注入 inlay，投影长度可能更大。
            original_len: text.len(),
            inlays,
            projected_base: base.projected,
            original_base: base.original,
            styles,
            fragment_range,
            style_index: 0,
            background_index: 0,
            marked_index: 0,
            dimmed_index: 0,
        }
    }

    fn original_offset(inlays: &[InlayInfo], projected: usize) -> usize {
        for inlay in inlays {
            if projected >= inlay.projected && projected < inlay.projected + inlay.text.len() {
                return inlay.anchor;
            }
        }
        projected
            - inlays
                .iter()
                .take_while(|inlay| inlay.projected + inlay.text.len() <= projected)
                .map(|inlay| inlay.text.len())
                .sum::<usize>()
    }

    fn to_original(&self, projected: usize) -> usize {
        Self::original_offset(self.inlays, projected).saturating_sub(self.original_base)
    }

    fn chunk_style(&mut self, start: usize, end: usize) -> Option<ChunkStyle> {
        if start < self.fragment_range.start || end > self.fragment_range.end {
            return None;
        }
        let is_inlay = self.inlays.iter().any(|inlay| {
            inlay.projected <= self.projected_base + start
                && self.projected_base + end <= inlay.projected + inlay.text.len()
        });
        let original_range = self.to_original(start)..self.to_original(end);
        while self
            .styles
            .spans
            .get(self.style_index)
            .is_some_and(|span| span.range.end <= self.global_byte_start + original_range.start)
        {
            self.style_index += 1;
        }
        let style = if is_inlay {
            None
        } else {
            self.styles.spans.get(self.style_index).and_then(|span| {
                let span_start = span
                    .range
                    .start
                    .saturating_sub(self.global_byte_start)
                    .min(self.original_len);
                let span_end = span
                    .range
                    .end
                    .saturating_sub(self.global_byte_start)
                    .min(self.original_len);
                (span_start < original_range.end && span_end > original_range.start)
                    .then(|| self.styles.styles.get(span.capture as usize).copied())
                    .flatten()
            })
        };
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
        let marked = !is_inlay
            && self
                .styles
                .marked
                .get(self.marked_index)
                .is_some_and(|range| {
                    let range_start = range
                        .start()
                        .get()
                        .saturating_sub(self.global_byte_start)
                        .min(self.original_len);
                    let range_end = range
                        .end()
                        .get()
                        .saturating_sub(self.global_byte_start)
                        .min(self.original_len);
                    range_start < original_range.end && range_end > original_range.start
                });
        let dimmed = !is_inlay
            && self
                .styles
                .dimmed
                .get(self.dimmed_index)
                .is_some_and(|range| {
                    let range_start = range
                        .start
                        .saturating_sub(self.global_byte_start)
                        .min(self.original_len);
                    let range_end = range
                        .end
                        .saturating_sub(self.global_byte_start)
                        .min(self.original_len);
                    range_start < original_range.end && range_end > original_range.start
                });
        // 背景覆盖层（搜索高亮）：仅当段完全位于命中区间内才着色。
        // 段与区间部分相交时返回 None，使 InlayChunks 的样式切分扫描在区间边界处切分出精确的子段，避免整段着色吞掉区间外的相邻字符（如紧邻的引号）。
        let background = (!is_inlay)
            .then(|| {
                self.styles
                    .backgrounds
                    .get(self.background_index)
                    .and_then(|(range, color)| {
                        let start = range
                            .start
                            .saturating_sub(self.global_byte_start)
                            .min(self.original_len);
                        let end = range
                            .end
                            .saturating_sub(self.global_byte_start)
                            .min(self.original_len);
                        (start <= original_range.start && original_range.end <= end)
                            .then_some(*color)
                    })
            })
            .flatten();
        Some(ChunkStyle {
            is_inlay,
            style,
            background,
            marked,
            dimmed,
        })
    }
}

/// 投影 inlay 行的源 chunk。
///
/// 普通 `TextChunks` 只能遍历已经物化的投影字符串。
/// 此游标交替借用源文本切片和 inlay 字符串，因此渲染与测量不必为了展开 inlay 而构造整行文本。
enum InlayTextChunks<'a> {
    Plain(SourceTextChunks<'a>),
    Injected(InjectedTextChunks<'a>),
}

impl<'a> InlayTextChunks<'a> {
    fn new(
        text: ChunkText<'a>,
        inlays: &'a [InlayInfo],
        original_base: usize,
        inject_inlays: bool,
    ) -> Self {
        if inject_inlays && !inlays.is_empty() {
            Self::Injected(InjectedTextChunks {
                source: SourceTextChunks::new(text),
                inlays,
                original_base,
                // 当前源段可能从一行中间开始（折叠 close 尾段）。
                // 已位于段起点之前的 inlay 属于前一个段，不能再次输出。
                inlay_index: inlays.partition_point(|inlay| inlay.anchor < original_base),
                inlay_offset: 0,
            })
        } else {
            Self::Plain(SourceTextChunks::new(text))
        }
    }
}

impl<'a> Iterator for InlayTextChunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Plain(chunks) => chunks.next(),
            Self::Injected(chunks) => chunks.next(),
        }
    }
}

struct InjectedTextChunks<'a> {
    source: SourceTextChunks<'a>,
    inlays: &'a [InlayInfo],
    original_base: usize,
    inlay_index: usize,
    inlay_offset: usize,
}

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

impl<'a> Iterator for InjectedTextChunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let inlay = self.inlays.get(self.inlay_index);
            let anchor = inlay
                .map(|inlay| {
                    inlay
                        .anchor
                        .saturating_sub(self.original_base)
                        .min(self.source.text.len())
                })
                .unwrap_or(self.source.text.len());

            if self.source.offset < anchor {
                return self.source.next_limited(anchor - self.source.offset);
            }

            let inlay = inlay?;
            if self.inlay_offset < inlay.text.len() {
                let mut end = (self.inlay_offset + CHUNK_SIZE).min(inlay.text.len());
                while !inlay.text.is_char_boundary(end) {
                    end -= 1;
                }
                let chunk = Chunk::from_text(&inlay.text[self.inlay_offset..end]);
                self.inlay_offset = end;
                return Some(chunk);
            }
            self.inlay_index += 1;
            self.inlay_offset = 0;
        }
    }
}

impl<'a> Iterator for InlayChunks<'a, '_> {
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
            head.is_inlay = chunk_style.is_inlay;
            head.style = chunk_style.style;
            head.background = chunk_style.background;
            head.marked = chunk_style.marked;
            head.dimmed = chunk_style.dimmed;
            return Some(head);
        }
    }
}

fn projected_prefix_metrics(
    text: ChunkText<'_>,
    inlays: &[InlayInfo],
    original_base: usize,
    inject_inlays: bool,
    projected_end: usize,
) -> (usize, usize) {
    let mut remaining = projected_end;
    let mut chars = 0;
    let mut utf16 = 0;
    let mut chunks = InlayTextChunks::new(text, inlays, original_base, inject_inlays);
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
    inlay: &'a InlaySnapshot,
    styles: HighlightStyles<'b>,
    fragment_range: Range<usize>,
    segment_index: usize,
    current: Option<(InlayChunks<'a, 'b>, bool)>,
}

impl<'a, 'b> FoldChunks<'a, 'b> {
    pub(super) fn new(
        segments: &'a [FoldRowSegment],
        inlay: &'a InlaySnapshot,
        styles: HighlightStyles<'b>,
        fragment_range: Range<usize>,
    ) -> Self {
        Self {
            segments,
            inlay,
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
            if let Some((chunks, is_placeholder)) = self.current.as_mut() {
                if let Some(mut chunk) = chunks.next() {
                    chunk.is_placeholder = *is_placeholder;
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

            let (chunks, is_placeholder) = match &segment.kind {
                FoldRowSegmentKind::Placeholder => (
                    InlayChunks::new(
                        ChunkText::Borrowed(FOLD_PLACEHOLDER),
                        0,
                        &[],
                        ChunkBase::ZERO,
                        HighlightStyles::default(),
                        clipped_start - segment.merged_range.start
                            ..clipped_end - segment.merged_range.start,
                        false,
                    ),
                    true,
                ),
                FoldRowSegmentKind::Text {
                    stream_line,
                    projected_range,
                } => {
                    let projected_start =
                        projected_range.start + clipped_start - segment.merged_range.start;
                    let projected_end =
                        projected_range.start + clipped_end - segment.merged_range.start;
                    let original_start =
                        self.inlay.to_original_offset(*stream_line, projected_start);
                    let original_end = self.inlay.to_original_offset(*stream_line, projected_end);
                    let line_range = self
                        .inlay
                        .line_byte_range(*stream_line)
                        .expect("折叠文本段必须位于当前快照内");
                    (
                        InlayChunks::new(
                            ChunkText::Virtual {
                                snapshot: self.inlay.buffer_snapshot(),
                                range: MultiBufferOffset::new(
                                    line_range.start.get() + original_start,
                                )
                                    ..MultiBufferOffset::new(line_range.start.get() + original_end),
                            },
                            line_range.start.get(),
                            self.inlay.line_inlays(*stream_line),
                            ChunkBase::new(projected_start, original_start),
                            self.styles,
                            projected_start..projected_end,
                            true,
                        ),
                        false,
                    )
                }
            };
            self.current = Some((chunks, is_placeholder));
        }
    }
}

fn fold_prefix_metrics(
    segments: &[FoldRowSegment],
    inlay: &InlaySnapshot,
    projected_end: usize,
) -> (usize, usize) {
    let mut chars = 0;
    let mut utf16 = 0;
    let chunks = FoldChunks::new(
        segments,
        inlay,
        HighlightStyles::default(),
        0..projected_end,
    );
    for chunk in chunks {
        chars += chunk.text.chars().count();
        utf16 += chunk.text.chars().map(char::len_utf16).sum::<usize>();
    }
    (chars, utf16)
}

/// Wrap 层：把一个显示片段裁剪到可见窗口，并将 Fold/Inlay 产出的 chunk 交给 Tab 层展开。
pub(crate) struct WrapChunks<'a, 'b> {
    chunks: WrapChunkSource<'a, 'b>,
    utf16_start: usize,
    remaining: usize,
}

enum WrapChunkSource<'a, 'b> {
    Fold(TabChunks<'a, FoldChunks<'a, 'b>>),
    Inlay(TabChunks<'a, InlayChunks<'a, 'b>>),
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
        let (prefix_chars, prefix_utf16) = if let Some(segments) = source.segments {
            fold_prefix_metrics(segments, source.inlay, fragment_start)
        } else {
            projected_prefix_metrics(
                source.text.clone(),
                source.inlay.line_inlays(source.stream_line),
                0,
                source.inject_inlays,
                fragment_start,
            )
        };
        let chunks = if let Some(segments) = source.segments {
            WrapChunkSource::Fold(TabChunks::from_chunks(
                FoldChunks::new(segments, source.inlay, styles, fragment_start..fragment_end),
                tab_width,
                prefix_chars,
            ))
        } else {
            let inlays = source.inlay.line_inlays(source.stream_line);
            WrapChunkSource::Inlay(TabChunks::from_chunks(
                InlayChunks::new(
                    source.text,
                    source.global_byte_start,
                    inlays,
                    ChunkBase::ZERO,
                    styles,
                    fragment_start..fragment_end,
                    source.inject_inlays,
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
            WrapChunkSource::Inlay(chunks) => chunks.next(),
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

/// 软换行片段信息：后续 wrap 片段显示为缩进续行。
#[derive(Debug, Clone, Copy)]
pub(crate) struct WrapRowInfo {
    pub(crate) indent: usize,
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
    if chunk.is_inlay {
        run.font.style = gpui::FontStyle::Italic;
        run.color.a *= 0.6;
        return run;
    }
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
/// 这样 Block/Fold/Wrap/Inlay 游标能够直接借用快照中的文本，不需要为了跨 `next` 调用保存而复制成 `String` 或 `Vec`。
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
    pub(crate) window_prefix: &'a str,
    pub(crate) fold_segments: Option<&'a [FoldRowSegment]>,
}

/// 连续显示行事件。文本 chunk 只能在本次回调中被消费，避免在流中保存自引用状态。
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

/// Block/Fold/Wrap/Inlay/样式的连续显示行流。
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
        let inlay = fold.inlay_snapshot();
        let stream_line = inlay.stream().buffer_to_stream(Line::new(source.line()));
        let mut range = byte_range.clone();
        let mut window_start_column = 0;
        let mut window_prefix = "";
        // 只有水平窗口需要随机访问完整投影行。
        // 普通滚动包括折叠行，都从源段连续输出，不为整行创建投影字符串。
        let needs_projected_text = self.window_columns.is_some();
        let text = if needs_projected_text {
            if segments.is_some() {
                fold.row_text(projected)
            } else {
                inlay.line_text(stream_line)
            }
        } else {
            None
        };
        if needs_projected_text && text.is_none() {
            return;
        }
        let Some(raw_range) = inlay.line_byte_range(stream_line) else {
            return;
        };
        let inject_inlays = segments.is_none() && !needs_projected_text;
        let projected_len = if let Some(segments) = segments.as_ref() {
            segments
                .last()
                .expect("折叠合并行必须至少包含一个段")
                .merged_range
                .end
        } else if inject_inlays {
            inlay
                .projected_line_len(stream_line)
                .expect("可见流行必须具有投影长度")
        } else {
            text.as_ref().expect("投影行必须具有文本").len()
        };
        if let Some((start, end)) = self.window_columns {
            let row_text = text.as_ref().expect("水平窗口必须读取投影行文本").as_ref();
            // 水平窗口的输入是显示列而非字节。
            // 必须沿与命中测试一致的 grapheme/tab 宽度规则转换；
            // 直接把列当作字节会让 CJK、tab 和行内提示把窗口以及光标 x 坐标错位。
            let start = byte_for_display_column(row_text, 0, start, self.tab_width);
            let end = byte_for_display_column(row_text, 0, end, self.tab_width);
            range.start = range.start.max(start);
            range.end = range.end.min(end);
            if range.start > byte_range.start {
                window_start_column =
                    display_column_for_byte(row_text, 0, range.start, self.tab_width);
                window_prefix = &row_text[..range.start];
            }
        }
        let budget = MAX_RENDERED_LINE_LEN.saturating_sub(*indent);
        let line_styles = self.styles.for_range(
            *global_byte_start
                ..global_byte_start
                    + inlay
                        .line_byte_range(stream_line)
                        .map_or(0, |range| range.end.get() - range.start.get()),
        );
        let text_ref = text.as_ref().map(|text| text.as_ref());
        let mut chunks = WrapChunks::new(
            ChunkSource {
                text: match text_ref {
                    Some(text) => ChunkText::Borrowed(text),
                    None => ChunkText::Virtual {
                        snapshot: inlay.buffer_snapshot(),
                        range: raw_range,
                    },
                },
                projected_len,
                global_byte_start: *global_byte_start,
                stream_line,
                segments: segments.as_ref().map(|segments| segments.as_slice()),
                inlay,
                inject_inlays,
            },
            self.tab_width,
            line_styles,
            range,
            budget,
        );
        on_row(DisplayRowEvent::Text {
            row: DisplayTextRow {
                row: row.index(),
                excerpt: row.excerpt(),
                source_line: source.line(),
                fragment_index: *fragment_index,
                indent: *indent,
                utf16_start: chunks.utf16_start(),
                window_start_column,
                window_prefix,
                fold_segments: segments.as_ref().map(|segments| segments.as_slice()),
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
mod tests {
    use super::*;

    /// 行文本的 chunk 迭代器（128 字节对齐）。
    pub(crate) struct TextChunks<'a> {
        text: &'a str,
        offset: usize,
    }

    impl<'a> TextChunks<'a> {
        pub(crate) fn new(text: &'a str) -> Self {
            Self { text, offset: 0 }
        }
    }

    impl<'a> Iterator for TextChunks<'a> {
        type Item = Chunk<'a>;

        fn next(&mut self) -> Option<Self::Item> {
            if self.offset >= self.text.len() {
                return None;
            }
            let mut end = (self.offset + CHUNK_SIZE).min(self.text.len());
            while !self.text.is_char_boundary(end) {
                end -= 1;
            }
            let chunk = Chunk::from_text(&self.text[self.offset..end]);
            self.offset = end;
            Some(chunk)
        }
    }

    pub(super) struct RenderChunks<'a> {
        pub(super) chunks: Vec<Chunk<'a>>,
        pub(super) utf16_start: usize,
    }

    pub(super) fn render_line_chunks<'a>(
        text: &'a str,
        tab_width: usize,
        global_byte_start: usize,
        inlays: &'a [InlayInfo],
        styles: HighlightStyles<'_>,
        fragment_range: Range<usize>,
    ) -> RenderChunks<'a> {
        let projected_len = text.len() + inlays.iter().map(|inlay| inlay.text.len()).sum::<usize>();
        let fragment_start = fragment_range.start.min(projected_len);
        let fragment_end = fragment_range.end.min(projected_len);
        let styled = InlayChunks::new(
            ChunkText::Borrowed(text),
            global_byte_start,
            inlays,
            ChunkBase::ZERO,
            styles,
            fragment_start..fragment_end,
            true,
        );
        let (prefix_chars, prefix_utf16) =
            projected_prefix_metrics(ChunkText::Borrowed(text), inlays, 0, true, fragment_start);
        let chunks = TabChunks::from_chunks(styled, tab_width, prefix_chars).collect();
        RenderChunks {
            chunks,
            utf16_start: prefix_utf16,
        }
    }

    #[test]
    fn clips_shaped_line_at_utf8_boundary() {
        let text = format!("{}文", "a".repeat(MAX_RENDERED_LINE_LEN - 1));
        let mut chunks = TextChunks::new(&text);
        let first = chunks.next().expect("文本应产生第一个 chunk");
        assert_eq!(first.text.len(), CHUNK_SIZE);
        assert!(first.text.is_char_boundary(first.text.len()));
    }

    fn expand_tabs(text: &str, tab_width: usize, start_column: usize) -> Vec<Chunk<'_>> {
        TabChunks::from_chunks(TextChunks::new(text), tab_width, start_column).collect()
    }

    fn chunks_to_runs(chunks: &[Chunk<'_>], base: gpui::TextRun) -> Vec<gpui::TextRun> {
        chunks
            .iter()
            .map(|chunk| chunk_to_run(chunk, base.clone()))
            .collect()
    }

    #[test]
    fn from_text_marks_char_starts_and_tabs() {
        let chunk = Chunk::from_text("a\t你😀");
        // 字符起始字节：a(0) tab(1) 你(2) 😀(5)
        assert_eq!(chunk.chars, 0b0000_0000_0010_0111);
        assert_eq!(chunk.tabs, 0b10);
        assert_eq!(chunk.chars_before_tab(1), 1);
    }

    #[test]
    fn split_at_shifts_bitmaps_at_a_char_boundary() {
        let chunk = Chunk::from_text("a你😀");
        let (left, right) = chunk.split_at(1);
        assert_eq!(left.text, "a");
        assert_eq!(left.chars, 0b1);
        assert_eq!(right.text, "你😀");
        assert_eq!(right.chars, 0b1001);
    }

    #[test]
    #[should_panic(expected = "chunk transforms must split at a UTF-8 character boundary")]
    fn split_at_rejects_a_non_boundary_instead_of_repairing_it() {
        Chunk::from_text("a你😀").split_at(2);
    }

    #[test]
    fn split_at_chunk_capacity_returns_an_empty_suffix() {
        let text = "a".repeat(CHUNK_SIZE);
        let (left, right) = Chunk::from_text(&text).split_at(CHUNK_SIZE);
        assert_eq!(left.text, text);
        assert_eq!(left.chars, u128::MAX);
        assert!(right.text.is_empty());
        assert_eq!(right.chars, 0);
    }

    #[test]
    fn text_chunks_emit_128_byte_aligned_pieces() {
        let text = "a".repeat(300);
        let chunks: Vec<_> = TextChunks::new(&text).collect();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].text.len(), 128);
        assert_eq!(chunks[1].text.len(), 128);
        assert_eq!(chunks[2].text.len(), 44);
        for chunk in &chunks {
            // len=128 时全 1（u128 上限）；否则低 len 位为 1。
            let expected = if chunk.text.len() < 128 {
                (1u128 << chunk.text.len()) - 1
            } else {
                u128::MAX
            };
            assert_eq!(chunk.chars, expected, "len={}", chunk.text.len());
        }
    }

    #[test]
    fn text_chunks_split_at_utf8_boundary() {
        // 128 字节切分点落在多字节字符中间时向左修正。
        let text = format!("{}你{}", "a".repeat(127), "b".repeat(100));
        let chunks: Vec<_> = TextChunks::new(&text).collect();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text.len(), 127);
        assert!(chunks[0].text.ends_with('a'));
        assert!(chunks[1].text.starts_with('你'));
    }

    #[test]
    fn style_coordinates_never_become_text_slice_offsets() {
        let text = "abc机def";
        let style = HighlightStyle {
            color: Some(gpui::red()),
            ..Default::default()
        };
        let line = render_line_chunks(
            text,
            4,
            0,
            &[],
            HighlightStyles {
                // 结束位置 4 落在“机”的 UTF-8 编码中间。
                backgrounds: &[],
                spans: &[HighlightSpan {
                    range: 0..4,
                    capture: 0,
                }],
                styles: &[style],
                marked: &[],
                dimmed: &[],
            },
            0..text.len(),
        );
        assert_eq!(
            line.chunks
                .iter()
                .map(|chunk| chunk.text)
                .collect::<String>(),
            text
        );
    }

    #[test]
    fn tab_expanded_chunks_expand_tabs_with_is_tab_markers() {
        let chunks = expand_tabs("a\tb", 4, 0);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].text, "a");
        assert!(!chunks[0].is_tab);
        // "a" 后 tab：列 1 对齐到 4 → 3 空格。
        assert_eq!(chunks[1].text, "   ");
        assert!(chunks[1].is_tab);
        assert_eq!(chunks[2].text, "b");
        assert!(!chunks[2].is_tab);
    }

    #[test]
    fn tab_expanded_chunks_align_to_tab_stops_with_start_column() {
        // 行首 tab：列 0 对齐到 4 → 4 空格。
        let chunks = expand_tabs("\ta", 4, 0);
        assert_eq!(chunks[0].text, "    ");
        assert!(chunks[0].is_tab);
        // 列 2 处的 tab → 2 空格（对齐到 4 的 tab stop）。
        let chunks = expand_tabs("ab\tc", 4, 0);
        assert_eq!(chunks[1].text, "  ");
        assert!(chunks[1].is_tab);
        // 恰在 tab stop（列 4）处的 tab → 4 空格。
        let chunks = expand_tabs("abcd\t", 4, 0);
        assert_eq!(chunks[1].text, "    ");
        // 起始列 1：tab 从列 1 对齐 → 3 空格。
        let chunks = expand_tabs("\tx", 4, 1);
        assert_eq!(chunks[0].text, "   ");
    }

    #[test]
    fn tab_expanded_chunks_pass_through_without_tabs() {
        let chunks = expand_tabs("hello", 4, 0);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "hello");
        assert!(!chunks[0].is_tab);
    }

    #[test]
    fn chunk_pipeline_splits_styles_before_tab_expansion() {
        let style = HighlightStyle {
            color: Some(gpui::red()),
            ..Default::default()
        };
        let line = render_line_chunks(
            "ab\tc",
            4,
            0,
            &[],
            HighlightStyles {
                backgrounds: &[],
                spans: &[HighlightSpan {
                    range: 0..3, // "ab\t"（3 个原字符）
                    capture: 0,
                }],
                styles: &[style],
                marked: &[],
                dimmed: &[],
            },
            0..5,
        );
        // span 端点（原始字节 3）处切分：样式段（含 tab 展开的两个 chunk）与无样式段。
        assert_eq!(line.chunks.len(), 3);
        assert_eq!(line.chunks[0].text, "ab");
        assert!(line.chunks[0].style.is_some());
        assert_eq!(line.chunks[1].text, "  ");
        assert!(line.chunks[1].is_tab);
        assert!(line.chunks[1].style.is_some());
        assert_eq!(line.chunks[2].text, "c");
        assert!(line.chunks[2].style.is_none());
    }

    #[test]
    fn chunk_pipeline_marks_selected_ranges() {
        let line = render_line_chunks(
            "abcdef",
            4,
            0,
            &[],
            HighlightStyles {
                backgrounds: &[],
                spans: &[],
                styles: &[],
                marked: &[MultiBufferRange::new(
                    MultiBufferOffset::new(2),
                    MultiBufferOffset::new(4),
                )
                .unwrap()],
                dimmed: &[],
            },
            0..6,
        );
        let marked = line
            .chunks
            .iter()
            .find(|chunk| chunk.marked)
            .expect("应有 marked 段");
        assert_eq!(marked.text, "cd");
    }

    #[test]
    fn chunks_to_runs_preserves_unicode_lengths_and_marked_style() {
        let text = "a中文b";
        let line = render_line_chunks(
            text,
            4,
            0,
            &[],
            HighlightStyles {
                backgrounds: &[],
                spans: &[],
                styles: &[],
                marked: &[MultiBufferRange::new(
                    MultiBufferOffset::new(1),
                    MultiBufferOffset::new(7),
                )
                .unwrap()],
                dimmed: &[],
            },
            0..text.len(),
        );
        let runs = chunks_to_runs(
            &line.chunks,
            gpui::TextRun {
                len: 0,
                font: gpui::font("Helvetica"),
                color: Default::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            },
        );

        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
        assert_eq!(runs.len(), 3);
        assert!(runs[0].underline.is_none());
        assert!(runs[1].underline.is_some());
        assert!(runs[2].underline.is_none());
    }

    #[test]
    fn chunks_to_runs_fades_dimmed_ranges() {
        let text = "abc";
        let line = render_line_chunks(
            text,
            4,
            0,
            &[],
            HighlightStyles {
                spans: &[],
                styles: &[],
                backgrounds: &[],
                marked: &[],
                dimmed: std::slice::from_ref(&(1..2)),
            },
            0..text.len(),
        );
        let runs = chunks_to_runs(
            &line.chunks,
            gpui::TextRun {
                len: 0,
                font: gpui::font("Helvetica"),
                color: gpui::white(),
                background_color: None,
                underline: None,
                strikethrough: None,
            },
        );

        assert_eq!(runs.len(), 3);
        assert!(runs[1].color.a < runs[0].color.a);
        assert_eq!(runs[2].color.a, runs[0].color.a);
    }

    #[test]
    fn chunk_pipeline_marks_inlays_after_anchor_characters() {
        // inlay（锚定偏移 1 处）注入 "ab" 的投影文本。
        let inlays = [InlayInfo {
            anchor: 1,
            projected: 1,
            text: Arc::from(": hint"),
        }];
        let line = render_line_chunks("a: hintb", 4, 0, &inlays, HighlightStyles::default(), 0..8);
        // 段：0..1（"a"）+ 1..7（": hint"，inlay）+ 7..8（"b"）。
        assert_eq!(line.chunks.len(), 3);
        let inlay = line
            .chunks
            .iter()
            .find(|chunk| chunk.is_inlay)
            .expect("应有 inlay 段");
        assert_eq!(inlay.text, ": hint");
        assert!(!line.chunks[0].is_inlay && !line.chunks[2].is_inlay);
        assert!(line.chunks.iter().all(|chunk| chunk.style.is_none()));
    }

    #[test]
    fn chunk_pipeline_crops_inlay_segments_to_fragment() {
        // 片段裁剪：inlay 段被片段边界切开，样式判定按片段内范围。
        let inlays = [InlayInfo {
            anchor: 1,
            projected: 1,
            text: Arc::from(": hint"),
        }];
        let line = render_line_chunks("a: hintb", 4, 0, &inlays, HighlightStyles::default(), 1..5);
        assert_eq!(line.chunks.len(), 1);
        assert_eq!(line.chunks[0].text, ": hi");
        assert!(line.chunks[0].is_inlay);
        assert_eq!(line.utf16_start, 1);
    }

    #[test]
    fn chunk_pipeline_maps_span_boundaries_through_inlay_prefix() {
        // span 端点（原始坐标）经 inlay 前缀映射到投影偏移切分。
        let style = HighlightStyle {
            color: Some(gpui::red()),
            ..Default::default()
        };
        let inlays = [InlayInfo {
            anchor: 1,
            projected: 1,
            text: Arc::from(": hint"),
        }];
        let line = render_line_chunks(
            "ab",
            4,
            0,
            &inlays,
            HighlightStyles {
                backgrounds: &[],
                spans: &[HighlightSpan {
                    range: 1..2, // 原始 1..2 = "b"（inlay 注入后右移）
                    capture: 0,
                }],
                styles: &[style],
                marked: &[],
                dimmed: &[],
            },
            0..8,
        );
        let styled = line
            .chunks
            .iter()
            .find(|chunk| chunk.style.is_some())
            .expect("应有样式段");
        assert_eq!(styled.text, "b");
    }

    #[test]
    fn chunk_pipeline_clips_span_boundaries_to_line() {
        // span 端点 clip 到行内：行外 span 不产生额外切分。
        let style = HighlightStyle {
            color: Some(gpui::red()),
            ..Default::default()
        };
        let line = render_line_chunks(
            "abc",
            4,
            10,
            &[],
            HighlightStyles {
                backgrounds: &[],
                spans: &[HighlightSpan {
                    range: 0..100,
                    capture: 0,
                }],
                styles: &[style],
                marked: &[],
                dimmed: &[],
            },
            0..3,
        );
        assert_eq!(line.chunks.len(), 1);
        assert_eq!(line.chunks[0].text, "abc");
        assert!(line.chunks[0].style.is_some());
    }
}

#[cfg(test)]
mod backgrounds_layer_tests {
    use super::tests::render_line_chunks;
    use super::*;
    use gpui::rgba;

    /// 背景覆盖层：命中区间内所有 chunk 都带背景色（搜索高亮普通/活动匹配共用此层）。
    #[test]
    fn backgrounds_layer_colors_all_matching_chunks() {
        let text = "abc abc";
        let line = render_line_chunks(
            text,
            4,
            0,
            &[],
            HighlightStyles {
                spans: &[],
                styles: &[],
                backgrounds: &[(0..3, rgba(0x74ade83d)), (4..7, rgba(0x74ade8b3))],
                marked: &[],
                dimmed: &[],
            },
            0..text.len(),
        );
        let chunks = line.chunks;
        assert_eq!(chunks.len(), 3, "两个匹配 + 中间空格各自成段");
        // 普通匹配（0-3）与活动匹配（4-7）背景色不同且都命中。
        assert_eq!(chunks[0].background, Some(rgba(0x74ade83d)));
        assert_eq!(chunks[1].background, None, "无背景区间不应被着色");
        assert_eq!(chunks[2].background, Some(rgba(0x74ade8b3)));
    }

    /// 匹配紧邻引号（同一语法段）时，背景不得吞掉区间外的引号字符。
    #[test]
    fn backgrounds_do_not_spill_into_adjacent_quotes() {
        let text = "\"abc\" abc";
        // 匹配区间 1..4（abc），引号在 0 与 4。
        let line = render_line_chunks(
            text,
            4,
            0,
            &[],
            HighlightStyles {
                spans: &[],
                styles: &[],
                backgrounds: &[(1..4, rgba(0x74ade83d))],
                marked: &[],
                dimmed: &[],
            },
            0..text.len(),
        );
        let chunks = line.chunks;
        assert_eq!(
            chunks.len(),
            3,
            "引号 / 匹配 / 空格后文本应切分为三段的精确子段"
        );
        assert_eq!(chunks[0].text, "\"", "左引号单独成段");
        assert_eq!(chunks[0].background, None, "左引号不应着色");
        assert_eq!(chunks[1].text, "abc");
        assert_eq!(
            chunks[1].background,
            Some(rgba(0x74ade83d)),
            "匹配词本身着色"
        );
    }
}
