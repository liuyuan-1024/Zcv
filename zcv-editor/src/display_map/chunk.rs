//! 字节级 chunk：文本切片 + 字符/tab 位图（对齐 Zed rope Chunk 的位图语义）。
//!
//! 位图让"任意字节边界切分"与"tab 展开/字符坐标换算"变成 O(1) 位运算，无需逐字符扫描：
//! - `chars`：每个 UTF-8 字符的起始字节 bit=1（LSB 对应文本字节 0）；
//! - `tabs`：每个 tab 字节 bit=1。
//!
//! 渲染层按行消费 chunk 流（128 字节对齐，与 Zed rope chunk 上限一致）；
//! 跨行的换行位图在 Zed 中由 `newlines` 承担，当前行级渲染不需要，裁掉。
//!
//! 渲染对齐 Zed highlighted_chunks：基础文本 chunk 经 inlay、样式与 tab 变换，产出带样式与 is_tab/is_inlay 标记的渲染 chunk；渲染端逐 chunk 生成 TextRun 后统一 shape。

use std::ops::Range;

use gpui::{HighlightStyle, UnderlineStyle, px};
use zcv_engine::{Line, TextRange};
use zcv_language::HighlightSpan;

use super::DisplaySnapshot;
use super::fold_map::{FoldRowSegment, FoldRowSegmentKind};
use super::inlay_map::InlaySnapshot;
use super::line_stream::StreamLineSource;
use super::wrap_map::WrapViewportRowKind;
use zcv_theme::color;

/// chunk 文本字节上限（与 Zed rope Chunk 的 MAX_BASE 一致）。
pub(crate) const CHUNK_SIZE: usize = 128;

/// 行内提示（inlay）的显示信息：注入在投影文本中的一段文本。
///
/// 锚定字符之后的原始行内字节偏移，与注入后（含此前所有注入文本）的投影偏移；
/// 渲染合成与偏移换算共用同一信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlayInfo<'a> {
    pub(crate) anchor: usize,
    pub(crate) projected: usize,
    pub(crate) text: &'a str,
}

/// 渲染 chunk：文本切片 + 字符/tab 位图 + 样式标记（对齐 Zed Chunk 的 is_tab/is_inlay/highlight_style）。
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
    /// 与 Zed 的 `rope::ChunkSlice::split_at` 一样，本方法不修正调用者给出的坐标：
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

/// 静态空格表（tab 展开的空格段借用它；tab 宽度对齐的跨度 ≤ tab_width）。
const SPACES: &str = "                                                                ";

/// 展开 tab 后的 chunk 流（tabs 位图驱动，对齐 Zed TabChunks）。
///
/// 展开与测量（`advance_display_column`）同规则：tab 宽度 = `tab_width - col % tab_width`；
/// 展开前的起始列作为片段内展开的列对齐基准。
/// 输出的 tab 段标记 `is_tab`，文本借用静态空格表。
struct TabExpandedChunks<'a, I>
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

impl<'a, I> TabExpandedChunks<'a, I>
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

impl<'a, I> Iterator for TabExpandedChunks<'a, I>
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

/// 一次显示片段的渲染 chunk。
///
/// 投影行文本（含行内提示注入的文本；行尾换行未剥时片段裁剪会排除）；
/// 行内提示的注入信息（按锚定偏移排序）；软换行片段在投影文本内的范围；
/// 行首的原始 buffer 字节（spans/marked 的坐标域）。
/// 展开后的字符列 = 显示列（tab 展开成空格，shaping 宽度与测量一致）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RenderChunks<'a> {
    pub(crate) chunks: Vec<Chunk<'a>>,
    /// 片段起点前的投影字符数（光标/命中测试的 UTF-16 起点；不含 wrap 假空格）。
    pub(crate) utf16_start: usize,
}

/// 行的样式输入（语法高亮 + 搜索背景层 + 选区标记）。
///
/// 独立于语法前景色的背景覆盖层（对齐 Zed 的 background highlights）：
/// 搜索匹配等只改背景、保留语法前景色的场景走这一层，不经过 spans 的 style 替换。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LineStyles<'a> {
    pub(crate) spans: &'a [HighlightSpan],
    pub(crate) styles: &'a [HighlightStyle],
    /// 背景覆盖层：命中区间优先于语法 style 的背景色。
    pub(crate) backgrounds: &'a [(Range<usize>, gpui::Rgba)],
    pub(crate) marked: &'a [TextRange],
}

pub(crate) struct ViewportChunkSource<'a> {
    pub text: &'a str,
    pub global_byte_start: usize,
    pub stream_line: Line,
    pub segments: Option<&'a [FoldRowSegment]>,
    pub inlay: &'a InlaySnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ChunkStyle {
    is_inlay: bool,
    style: Option<HighlightStyle>,
    /// 背景覆盖层命中色（搜索高亮等；优先于 style 的背景）。
    background: Option<gpui::Rgba>,
    marked: bool,
}

/// Zed 式样式变换：输入只能是 `TextChunks` 产生的安全 chunk，输出也只能在输入 chunk 的字符位图边界处分段。
/// 高亮、选区和 inlay 坐标只决定段的元数据，不直接作为 `str` 的切片下标。
struct StyledChunks<'a, 'b> {
    source: TextChunks<'a>,
    current: Option<Chunk<'a>>,
    projected_offset: usize,
    global_byte_start: usize,
    original_len: usize,
    inlays: &'b [InlayInfo<'a>],
    styles: LineStyles<'b>,
    fragment_range: Range<usize>,
}

impl<'a, 'b> StyledChunks<'a, 'b> {
    fn new(
        text: &'a str,
        global_byte_start: usize,
        inlays: &'b [InlayInfo<'a>],
        styles: LineStyles<'b>,
        fragment_range: Range<usize>,
    ) -> Self {
        Self {
            source: TextChunks::new(text),
            current: None,
            projected_offset: 0,
            global_byte_start,
            original_len: text.len() - inlays.iter().map(|inlay| inlay.text.len()).sum::<usize>(),
            inlays,
            styles,
            fragment_range,
        }
    }

    fn to_original(&self, projected: usize) -> usize {
        for inlay in self.inlays {
            if projected >= inlay.projected && projected < inlay.projected + inlay.text.len() {
                return inlay.anchor;
            }
        }
        projected
            - self
                .inlays
                .iter()
                .take_while(|inlay| inlay.projected + inlay.text.len() <= projected)
                .map(|inlay| inlay.text.len())
                .sum::<usize>()
    }

    fn chunk_style(&self, start: usize, end: usize) -> Option<ChunkStyle> {
        if start < self.fragment_range.start || end > self.fragment_range.end {
            return None;
        }
        let is_inlay = self
            .inlays
            .iter()
            .any(|inlay| inlay.projected <= start && end <= inlay.projected + inlay.text.len());
        let original_range = self.to_original(start)..self.to_original(end);
        let style = (!is_inlay)
            .then(|| {
                self.styles.spans.iter().find_map(|span| {
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
            })
            .flatten();
        let marked = !is_inlay
            && self.styles.marked.iter().any(|range| {
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
        // 背景覆盖层（搜索高亮）：仅当段完全位于命中区间内才着色。
        // 段与区间部分相交时返回 None，使 StyledChunks 的样式切分扫描在区间边界处切分出精确的子段，避免整段着色吞掉区间外的相邻字符（如紧邻的引号）。
        let background = (!is_inlay)
            .then(|| {
                self.styles.backgrounds.iter().find_map(|(range, color)| {
                    let start = range
                        .start
                        .saturating_sub(self.global_byte_start)
                        .min(self.original_len);
                    let end = range
                        .end
                        .saturating_sub(self.global_byte_start)
                        .min(self.original_len);
                    (start <= original_range.start && original_range.end <= end).then_some(*color)
                })
            })
            .flatten();
        Some(ChunkStyle {
            is_inlay,
            style,
            background,
            marked,
        })
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
            head.is_inlay = chunk_style.is_inlay;
            head.style = chunk_style.style;
            head.background = chunk_style.background;
            head.marked = chunk_style.marked;
            return Some(head);
        }
    }
}

fn utf16_units_before(text: &str, byte: usize) -> usize {
    text.char_indices()
        .take_while(|(offset, _)| *offset < byte)
        .map(|(_, character)| character.len_utf16())
        .sum()
}

fn chars_before(text: &str, byte: usize) -> usize {
    text.char_indices()
        .take_while(|(offset, _)| *offset < byte)
        .count()
}

/// 从安全基础 chunk 构建一个显示片段的渲染 chunk。
///
/// 样式、选区与 inlay 只作为事件标记参与流的分段，不能直接对文本切片。
fn render_line_chunks<'a>(
    text: &'a str,
    tab_width: usize,
    global_byte_start: usize,
    inlays: &[InlayInfo<'a>],
    styles: LineStyles<'_>,
    fragment_range: Range<usize>,
) -> RenderChunks<'a> {
    let fragment_start = fragment_range.start.min(text.len());
    let fragment_end = fragment_range.end.min(text.len());
    debug_assert!(text.is_char_boundary(fragment_start));
    debug_assert!(text.is_char_boundary(fragment_end));
    let styled = StyledChunks::new(
        text,
        global_byte_start,
        inlays,
        styles,
        fragment_start..fragment_end,
    );
    let chunks =
        TabExpandedChunks::from_chunks(styled, tab_width, chars_before(text, fragment_start))
            .collect();
    RenderChunks {
        chunks,
        utf16_start: utf16_units_before(text, fragment_start),
    }
}

/// 折叠合并行（anchor 文本 + 占位符 + 闭合行尾段）的 chunk 合成。
///
/// 合并行内的高亮坐标域是断开的（anchor 行与 close 行是两个字节窗口），
/// 不能按单行窗口裁剪 spans，因此逐段调用行级合成：每段携带自己的行内提示
/// （偏移相对段起点）与全局字节基准，占位符段单独产出。
fn render_folded_chunks<'a>(
    merged: &'a str,
    tab_width: usize,
    segments: &[FoldRowSegment],
    inlay: &'a InlaySnapshot,
    styles: LineStyles<'_>,
    fragment_range: Range<usize>,
) -> RenderChunks<'a> {
    let fragment_start = fragment_range.start.min(merged.len());
    let fragment_end = fragment_range.end.min(merged.len());
    debug_assert!(merged.is_char_boundary(fragment_start));
    debug_assert!(merged.is_char_boundary(fragment_end));
    let mut styled_chunks = Vec::new();
    for segment in segments {
        let clipped_start = fragment_start
            .max(segment.merged_range.start)
            .min(segment.merged_range.end);
        let clipped_end = fragment_end
            .max(segment.merged_range.start)
            .min(segment.merged_range.end);
        if clipped_start >= clipped_end {
            continue;
        }
        let segment_text = &merged[segment.merged_range.clone()];
        let local =
            clipped_start - segment.merged_range.start..clipped_end - segment.merged_range.start;
        match &segment.kind {
            FoldRowSegmentKind::Placeholder => {
                styled_chunks.extend(
                    StyledChunks::new(segment_text, 0, &[], LineStyles::default(), local).map(
                        |mut chunk| {
                            chunk.is_placeholder = true;
                            chunk
                        },
                    ),
                );
            }
            FoldRowSegmentKind::Text {
                stream_line,
                projected_range,
                global_start,
            } => {
                // 段内注入：偏移相对段起点（锚定偏移同样相对段的原始起点）。
                let base_original = inlay.to_original_offset(*stream_line, projected_range.start);
                let segment_inlays: Vec<InlayInfo<'_>> = inlay
                    .line_inlays(*stream_line)
                    .into_iter()
                    .filter(|info| {
                        info.projected >= projected_range.start
                            && info.projected + info.text.len() <= projected_range.end
                    })
                    .map(|info| InlayInfo {
                        anchor: info.anchor - base_original,
                        projected: info.projected - projected_range.start,
                        text: info.text,
                    })
                    .collect();
                styled_chunks.extend(StyledChunks::new(
                    segment_text,
                    *global_start,
                    &segment_inlays,
                    styles,
                    local,
                ));
            }
        }
    }
    // Fold 先于 tab：anchor、占位符和 tail 的 chunk 合流后只做一次 tab 变换，因而 tab stop 不会在段边界重置。
    let chunks = TabExpandedChunks::from_chunks(
        styled_chunks.into_iter(),
        tab_width,
        chars_before(merged, fragment_start),
    )
    .collect();
    RenderChunks {
        chunks,
        utf16_start: utf16_units_before(merged, fragment_start),
    }
}

/// 单一 viewport chunk 入口。渲染端不区分普通行与折叠合并行；
/// 两者都从 display-map 的 inlay/fold 快照取得输入，并在这里进入同一条 chunk 变换链。
pub(crate) fn render_viewport_chunks<'a>(
    source: ViewportChunkSource<'a>,
    tab_width: usize,
    styles: LineStyles<'_>,
    fragment_range: Range<usize>,
) -> RenderChunks<'a> {
    if let Some(segments) = source.segments {
        render_folded_chunks(
            source.text,
            tab_width,
            segments,
            source.inlay,
            styles,
            fragment_range,
        )
    } else {
        let inlays = source.inlay.line_inlays(source.stream_line);
        render_line_chunks(
            source.text,
            tab_width,
            source.global_byte_start,
            &inlays,
            styles,
            fragment_range,
        )
    }
}

/// 软换行片段信息：后续 wrap 片段显示为缩进续行。
#[derive(Debug, Clone, Copy)]
pub(crate) struct WrapRowInfo {
    pub(crate) line: Line,
    pub(crate) indent: usize,
    pub(crate) column_base: usize,
}

/// 视口行的完整渲染数据（对齐 Zed highlighted_chunks 的封装目标）：
/// 行解构、四层快照链穿透、chunk 合成与 run 映射都在管线侧完成，渲染端只消费结果交给 shaping。
pub(crate) struct RenderedViewportRow {
    pub(crate) display_text: String,
    pub(crate) runs: Vec<gpui::TextRun>,
    pub(crate) utf16_start: usize,
    pub(crate) logical_line: Option<Line>,
    pub(crate) gutter_line: Option<Line>,
    pub(crate) wrap_info: Option<WrapRowInfo>,
    pub(crate) fold_segments: Option<Vec<FoldRowSegment>>,
}

/// 渲染一行的样式输入：语法高亮 / 搜索背景 / 标记范围，管线内组装为行样式。
pub(crate) struct RowStyleInput<'a> {
    pub(crate) visible_highlights: &'a [HighlightSpan],
    pub(crate) highlight_styles: &'a [HighlightStyle],
    pub(crate) search_backgrounds: &'a [(Range<usize>, gpui::Rgba)],
    pub(crate) marked_ranges: &'a [TextRange],
}

/// 渲染一行视口行。
///
/// 管线内完成行解构、inlay 快照穿透、stream 行换算、chunk 合成与 run 映射；样式输入与基础 run 由渲染端提供。
pub(crate) fn render_viewport_row(
    row: &WrapViewportRowKind<'_>,
    display_snapshot: &DisplaySnapshot,
    style_input: &RowStyleInput<'_>,
    base: gpui::TextRun,
    cx: &gpui::App,
) -> RenderedViewportRow {
    let WrapViewportRowKind::Text {
        source,
        text,
        byte_range,
        global_byte_start,
        fragment_index,
        indent,
        column_base,
        segments,
    } = row;
    // 行内提示（inlay）：经消费链查询行的注入段（投影偏移已含此前注入前缀）。
    let inlay_snapshot = display_snapshot
        .wrap_snapshot()
        .tab_snapshot()
        .fold_snapshot()
        .inlay_snapshot();
    let stream_line = match source {
        StreamLineSource::Buffer(buffer_line) => inlay_snapshot
            .stream()
            .buffer_to_stream(Line::new(*buffer_line)),
        StreamLineSource::Inserted { anchor, index } => {
            let start = inlay_snapshot
                .stream()
                .inserted_block_start(*anchor)
                .expect("合成行必须属于锚定块的插入表");
            Line::new(start.get() + index)
        }
    };
    // 合成行是外部文本：无语法高亮、不可编辑/不可选（spans/marked 是锚定行的 buffer 坐标，套用到合成行文本会产生非字符边界切片）。
    let line_styles = match source {
        StreamLineSource::Buffer(_) => LineStyles {
            spans: style_input.visible_highlights,
            styles: style_input.highlight_styles,
            backgrounds: style_input.search_backgrounds,
            marked: style_input.marked_ranges,
        },
        StreamLineSource::Inserted { .. } => LineStyles::default(),
    };
    let tab_width = display_snapshot.buffer_snapshot().config().tab.tab_width();
    let rendered = render_viewport_chunks(
        ViewportChunkSource {
            text: text.as_ref(),
            global_byte_start: *global_byte_start,
            stream_line,
            segments: segments.as_deref(),
            inlay: inlay_snapshot,
        },
        tab_width,
        line_styles,
        byte_range.clone(),
    );
    // 显示文本：wrap 假空格 + 展开 chunk 文本拼接（对齐 Zed from_chunks）。
    let display_len: usize = *indent
        + rendered
            .chunks
            .iter()
            .map(|chunk| chunk.text.len())
            .sum::<usize>();
    let mut display_text = String::with_capacity(display_len);
    if *indent > 0 {
        display_text.push_str(&" ".repeat(*indent));
    }
    for chunk in &rendered.chunks {
        display_text.push_str(chunk.text);
    }
    let mut runs = Vec::with_capacity(rendered.chunks.len() + 1);
    if *indent > 0 {
        runs.push(gpui::TextRun {
            len: *indent,
            ..base.clone()
        });
    }
    let mut chunk_runs = chunks_to_runs(&rendered.chunks, base);
    // 折叠占位符用占位色绘制。
    for (run, chunk) in chunk_runs.iter_mut().zip(&rendered.chunks) {
        if chunk.is_placeholder {
            run.color = color::current(cx).text_placeholder.into();
        }
    }
    runs.extend(chunk_runs);
    let utf16_start = rendered.utf16_start;
    let logical_line = match source {
        StreamLineSource::Buffer(buffer_line) => Some(Line::new(*buffer_line)),
        StreamLineSource::Inserted { .. } => None,
    };
    let wrap_info = (*fragment_index > 0).then_some(WrapRowInfo {
        line: logical_line.unwrap_or(Line::ZERO),
        indent: *indent,
        column_base: *column_base,
    });
    // 行号只在逻辑行首显示行出现。
    let gutter_line = match source {
        StreamLineSource::Buffer(buffer_line) if *fragment_index == 0 => {
            Some(Line::new(*buffer_line))
        }
        _ => None,
    };
    RenderedViewportRow {
        display_text,
        runs,
        utf16_start,
        logical_line,
        gutter_line,
        wrap_info,
        fold_segments: segments.clone(),
    }
}

/// 渲染端把 chunk 流转成 TextRun（对齐 Zed from_chunks：每 chunk 一个 run，base 合并样式）。
pub(crate) fn chunks_to_runs(chunks: &[Chunk<'_>], base: gpui::TextRun) -> Vec<gpui::TextRun> {
    chunks
        .iter()
        .map(|chunk| {
            let mut run = gpui::TextRun {
                len: chunk.text.len(),
                ..base.clone()
            };
            if chunk.is_inlay {
                // 行内提示：斜体 + 半透明（对齐 Zed inlay_hint 的呈现）。
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
            run
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zcv_engine::ByteOffset;

    fn expand_tabs(text: &str, tab_width: usize, start_column: usize) -> Vec<Chunk<'_>> {
        TabExpandedChunks::from_chunks(TextChunks::new(text), tab_width, start_column).collect()
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
            LineStyles {
                // 结束位置 4 落在“机”的 UTF-8 编码中间。
                backgrounds: &[],
                spans: &[HighlightSpan {
                    range: 0..4,
                    capture: 0,
                }],
                styles: &[style],
                marked: &[],
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
            LineStyles {
                backgrounds: &[],
                spans: &[HighlightSpan {
                    range: 0..3, // "ab\t"（3 个原字符）
                    capture: 0,
                }],
                styles: &[style],
                marked: &[],
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
            LineStyles {
                backgrounds: &[],
                spans: &[],
                styles: &[],
                marked: &[TextRange::new(ByteOffset::new(2), ByteOffset::new(4)).unwrap()],
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
            LineStyles {
                backgrounds: &[],
                spans: &[],
                styles: &[],
                marked: &[TextRange::new(ByteOffset::new(1), ByteOffset::new(7)).unwrap()],
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
    fn chunk_pipeline_marks_inlays_after_anchor_characters() {
        // inlay（锚定偏移 1 处）注入 "ab" 的投影文本。
        let line = render_line_chunks(
            "a: hintb",
            4,
            0,
            &[InlayInfo {
                anchor: 1,
                projected: 1,
                text: ": hint",
            }],
            LineStyles::default(),
            0..8,
        );
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
        let line = render_line_chunks(
            "a: hintb",
            4,
            0,
            &[InlayInfo {
                anchor: 1,
                projected: 1,
                text: ": hint",
            }],
            LineStyles::default(),
            1..5,
        );
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
        let line = render_line_chunks(
            "a: hintb",
            4,
            0,
            &[InlayInfo {
                anchor: 1,
                projected: 1,
                text: ": hint",
            }],
            LineStyles {
                backgrounds: &[],
                spans: &[HighlightSpan {
                    range: 1..2, // 原始 1..2 = "b"（inlay 注入后右移）
                    capture: 0,
                }],
                styles: &[style],
                marked: &[],
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
            LineStyles {
                backgrounds: &[],
                spans: &[HighlightSpan {
                    range: 0..100,
                    capture: 0,
                }],
                styles: &[style],
                marked: &[],
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
            LineStyles {
                spans: &[],
                styles: &[],
                backgrounds: &[(0..3, rgba(0x74ade83d)), (4..7, rgba(0x74ade8b3))],
                marked: &[],
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
            LineStyles {
                spans: &[],
                styles: &[],
                backgrounds: &[(1..4, rgba(0x74ade83d))],
                marked: &[],
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
