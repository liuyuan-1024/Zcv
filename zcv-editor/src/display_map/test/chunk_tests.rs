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
}

pub(super) fn render_line_chunks<'a>(
    text: &'a str,
    tab_width: usize,
    global_byte_start: usize,
    styles: HighlightStyles<'_>,
    fragment_range: Range<usize>,
) -> RenderChunks<'a> {
    let projected_len = text.len();
    let fragment_start = fragment_range.start.min(projected_len);
    let fragment_end = fragment_range.end.min(projected_len);
    let styled = StyledChunks::new(
        ChunkText::Borrowed(text),
        global_byte_start,
        0,
        styles,
        fragment_start..fragment_end,
    );
    let (prefix_chars, _) = prefix_metrics(ChunkText::Borrowed(text), fragment_start);
    let chunks = TabChunks::from_chunks(styled, tab_width, prefix_chars).collect();
    RenderChunks { chunks }
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
        HighlightStyles {
            backgrounds: &[],
            spans: &[],
            styles: &[],
            marked: &[
                MultiBufferRange::new(MultiBufferOffset::new(2), MultiBufferOffset::new(4))
                    .unwrap(),
            ],
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
        HighlightStyles {
            backgrounds: &[],
            spans: &[],
            styles: &[],
            marked: &[
                MultiBufferRange::new(MultiBufferOffset::new(1), MultiBufferOffset::new(7))
                    .unwrap(),
            ],
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
