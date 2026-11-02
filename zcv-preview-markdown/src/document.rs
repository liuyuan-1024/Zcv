use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::{
    collections::{HashMap, HashSet},
    mem,
    ops::Range,
};
use zcv_language::SnippetHighlights;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InlineStyle {
    pub(crate) emphasis: bool,
    pub(crate) strong: bool,
    pub(crate) strikethrough: bool,
    pub(crate) code: bool,
    pub(crate) link: Option<String>,
    pub(crate) image: bool,
    pub(crate) math: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Inline {
    pub(crate) text: String,
    pub(crate) style: InlineStyle,
}

/// 一个在预览中渲染的块。
///
/// source 是块在源码组合文档（MultiBuffer 输出字节空间）中的来源区间，用于把预览几何
/// 与源码偏移关联：内部 #slug 锚点、大纲跳转和源码/预览滚动交接都依赖它。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Block {
    pub(crate) source: Range<usize>,
    pub(crate) kind: BlockKind,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BlockKind {
    Heading {
        level: u8,
        content: Vec<Inline>,
        slug: String,
    },
    Paragraph(Vec<Inline>),
    Code {
        language: Option<String>,
        text: String,
        highlights: Option<SnippetHighlights>,
    },
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Table {
        alignments: Vec<Alignment>,
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Image {
        source: String,
        alt: String,
    },
    Math {
        source: String,
        display: bool,
    },
    Mermaid {
        source: String,
    },
    Rule,
}

enum ActiveBlock {
    Heading {
        level: u8,
        source: Range<usize>,
    },
    Paragraph {
        source: Range<usize>,
    },
    Code {
        language: Option<String>,
        source: Range<usize>,
    },
}

enum Container {
    Quote {
        source: Range<usize>,
        blocks: Vec<Block>,
    },
    List {
        source: Range<usize>,
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Item(Vec<Block>),
}

struct Table {
    source: Range<usize>,
    alignments: Vec<Alignment>,
    in_header: bool,
    header: Vec<Vec<Inline>>,
    rows: Vec<Vec<Vec<Inline>>>,
    row: Vec<Vec<Inline>>,
    cell: Option<Vec<Inline>>,
}

struct ParseState {
    blocks: Vec<Block>,
    containers: Vec<Container>,
    table: Option<Table>,
}

impl ParseState {
    fn push_block(&mut self, block: Block) {
        match self.containers.last_mut() {
            Some(Container::Quote { blocks, .. }) | Some(Container::Item(blocks)) => {
                blocks.push(block)
            }
            Some(Container::List { .. }) | None => self.blocks.push(block),
        }
    }

    fn finish_quote(&mut self) {
        if let Some(Container::Quote { source, blocks }) = self.containers.pop() {
            self.push_block(Block {
                source,
                kind: BlockKind::Quote(blocks),
            });
        }
    }

    fn finish_list(&mut self) {
        if let Some(Container::List {
            source,
            start,
            items,
        }) = self.containers.pop()
        {
            self.push_block(Block {
                source,
                kind: BlockKind::List { start, items },
            });
        }
    }

    fn finish_item(&mut self) {
        let Some(Container::Item(item)) = self.containers.pop() else {
            return;
        };
        if let Some(Container::List { items, .. }) = self.containers.last_mut() {
            items.push(item);
        } else {
            self.blocks.extend(item);
        }
    }
}

pub(crate) fn parse(source: &str) -> Vec<Block> {
    let mut state = ParseState {
        blocks: Vec::new(),
        containers: Vec::new(),
        table: None,
    };
    let mut active = None;
    let mut active_content = Vec::new();
    let mut style = InlineStyle::default();
    let mut in_metadata_block = false;
    let mut options = Options::all();
    options.insert(Options::ENABLE_MATH);

    let NormalizedSource {
        text: normalized,
        math_blocks: display_math_blocks,
        offsets,
    } = extract_display_math_blocks(source);
    for (event, range) in Parser::new_ext(&normalized, options).into_offset_iter() {
        let source = offsets.range(range);
        match event {
            // 元数据只描述文档，不属于 Markdown 正文。
            // 必须在解析阶段丢弃，避免其文本被后续块（尤其是第一个标题）意外收集。
            Event::Start(Tag::MetadataBlock(_)) => in_metadata_block = true,
            Event::End(TagEnd::MetadataBlock(_)) => in_metadata_block = false,
            _ if in_metadata_block => {}
            Event::Start(Tag::Heading { level, .. }) => {
                finish_active(&mut active, &mut active_content, &mut state);
                active = Some(ActiveBlock::Heading {
                    level: heading_level(level),
                    source,
                });
            }
            Event::Start(Tag::Paragraph) if state.table.is_none() => {
                if active.is_none() {
                    active = Some(ActiveBlock::Paragraph { source });
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                finish_active(&mut active, &mut active_content, &mut state);
                active = Some(ActiveBlock::Code {
                    language: code_language(kind),
                    source,
                });
            }
            Event::InlineMath(math_source) => append_inline(
                &mut active_content,
                &mut state.table,
                &math_source,
                &InlineStyle {
                    math: true,
                    ..style.clone()
                },
            ),
            Event::DisplayMath(math_source) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.push_block(Block {
                    source,
                    kind: BlockKind::Math {
                        source: math_source.into_string(),
                        display: true,
                    },
                });
            }
            Event::Start(Tag::BlockQuote(_)) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.containers.push(Container::Quote {
                    source,
                    blocks: Vec::new(),
                });
            }
            Event::Start(Tag::List(start)) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.containers.push(Container::List {
                    source,
                    start,
                    items: Vec::new(),
                });
            }
            Event::Start(Tag::Item) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.containers.push(Container::Item(Vec::new()));
                active = Some(ActiveBlock::Paragraph { source });
            }
            Event::Start(Tag::Table(alignments)) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.table = Some(Table {
                    source,
                    alignments,
                    in_header: false,
                    header: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                    cell: None,
                });
            }
            Event::Start(Tag::TableHead) => {
                if let Some(table) = state.table.as_mut() {
                    table.in_header = true;
                }
            }
            Event::Start(Tag::TableRow) => {
                if let Some(table) = state.table.as_mut() {
                    table.row.clear();
                }
            }
            Event::Start(Tag::TableCell) => {
                if let Some(table) = state.table.as_mut() {
                    table.cell = Some(Vec::new());
                }
            }
            Event::Start(Tag::Emphasis) => style.emphasis = true,
            Event::Start(Tag::Strong) => style.strong = true,
            Event::Start(Tag::Strikethrough) => style.strikethrough = true,
            Event::Start(Tag::Link { dest_url, .. }) => style.link = Some(dest_url.into_string()),
            Event::Start(Tag::Image { dest_url, .. }) => {
                style.link = Some(dest_url.into_string());
                style.image = true;
                append_inline(&mut active_content, &mut state.table, "图片：", &style);
            }
            Event::End(TagEnd::Heading(_)) | Event::End(TagEnd::CodeBlock) => {
                finish_active(&mut active, &mut active_content, &mut state);
            }
            Event::End(TagEnd::Paragraph) if state.table.is_none() => {
                finish_active(&mut active, &mut active_content, &mut state);
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.finish_quote();
            }
            Event::End(TagEnd::Item) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.finish_item();
            }
            Event::End(TagEnd::List(_)) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.finish_list();
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(table) = state.table.as_mut()
                    && let Some(cell) = table.cell.take()
                {
                    table.row.push(cell);
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(table) = state.table.as_mut() {
                    let row = mem::take(&mut table.row);
                    if table.in_header {
                        table.header = row;
                    } else {
                        table.rows.push(row);
                    }
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(table) = state.table.as_mut() {
                    if table.header.is_empty() && !table.row.is_empty() {
                        table.header = mem::take(&mut table.row);
                    }
                    table.in_header = false;
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some(table) = state.table.take() {
                    state.push_block(Block {
                        source: table.source,
                        kind: BlockKind::Table {
                            alignments: table.alignments,
                            header: table.header,
                            rows: table.rows,
                        },
                    });
                }
            }
            Event::End(TagEnd::Emphasis) => style.emphasis = false,
            Event::End(TagEnd::Strong) => style.strong = false,
            Event::End(TagEnd::Strikethrough) => style.strikethrough = false,
            Event::End(TagEnd::Link) => style.link = None,
            Event::End(TagEnd::Image) => {
                style.link = None;
                style.image = false;
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                append_inline(&mut active_content, &mut state.table, &text, &style);
            }
            Event::Code(text) => {
                let mut code_style = style.clone();
                code_style.code = true;
                append_inline(&mut active_content, &mut state.table, &text, &code_style);
            }
            Event::SoftBreak => {
                append_inline(&mut active_content, &mut state.table, "\n", &style);
            }
            Event::HardBreak => {
                append_inline(&mut active_content, &mut state.table, " ", &style);
            }
            Event::TaskListMarker(done) => append_inline(
                &mut active_content,
                &mut state.table,
                if done { "[✓] " } else { "[ ] " },
                &style,
            ),
            Event::FootnoteReference(label) => append_inline(
                &mut active_content,
                &mut state.table,
                &format!("[{}]", label),
                &style,
            ),
            Event::Rule => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.push_block(Block {
                    source,
                    kind: BlockKind::Rule,
                });
            }
            _ => {}
        }
    }

    finish_active(&mut active, &mut active_content, &mut state);
    restore_display_math_blocks(&mut state.blocks, &display_math_blocks);
    assign_heading_slugs(&mut state.blocks);
    state.blocks
}

fn assign_heading_slugs(blocks: &mut [Block]) {
    fn visit(
        blocks: &mut [Block],
        counts: &mut HashMap<String, usize>,
        used: &mut HashSet<String>,
    ) {
        for block in blocks {
            match &mut block.kind {
                BlockKind::Heading { content, slug, .. } => {
                    let text = content
                        .iter()
                        .map(|inline| inline.text.as_str())
                        .collect::<String>();
                    let base = heading_slug(&text);
                    if base.is_empty() {
                        continue;
                    }
                    let count = counts.entry(base.clone()).or_default();
                    let mut candidate = if *count == 0 {
                        base.clone()
                    } else {
                        format!("{base}-{count}")
                    };
                    *count += 1;
                    while used.contains(&candidate) {
                        candidate = format!("{base}-{count}");
                        *count += 1;
                    }
                    used.insert(candidate.clone());
                    *slug = candidate;
                }
                BlockKind::Quote(children) => visit(children, counts, used),
                BlockKind::List { items, .. } => {
                    for item in items {
                        visit(item, counts, used);
                    }
                }
                _ => {}
            }
        }
    }
    visit(blocks, &mut HashMap::new(), &mut HashSet::new());
}

fn heading_slug(text: &str) -> String {
    text.trim()
        .chars()
        .filter_map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '-' | '_') {
                Some(ch.to_lowercase().next().unwrap_or(ch))
            } else if ch == ' ' {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

struct DisplayMathBlock {
    token: String,
    source: String,
    /// 公式块在原始源码中的区间。
    range: Range<usize>,
}

/// 归一化后的解析输入，以及归一化偏移回到原始源码偏移的映射。
struct NormalizedSource {
    text: String,
    math_blocks: Vec<DisplayMathBlock>,
    offsets: SourceOffsetMap,
}

/// 归一化文本的连续片段到原始源码起点的映射。
///
/// extract_display_math_blocks 会把多行 $$ 块压缩为单行占位标记，导致归一化文本与
/// 原始源码的字节偏移不再一致。每个写入片段记录其在两个坐标空间中的起点，即可把解析器
/// 给出的归一化区间还原为原始源码区间；片段内部按等长平移，跨片段边界处从目标片段重新取值。
struct SourceOffsetMap {
    segments: Vec<OffsetSegment>,
}

struct OffsetSegment {
    normalized_start: usize,
    original_start: usize,
}

impl SourceOffsetMap {
    fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    fn begin(&mut self, normalized_start: usize, original_start: usize) {
        self.segments.push(OffsetSegment {
            normalized_start,
            original_start,
        });
    }

    fn to_original(&self, offset: usize) -> usize {
        match self
            .segments
            .binary_search_by(|segment| segment.normalized_start.cmp(&offset))
        {
            Ok(index) => self.segments[index].original_start,
            Err(0) => self
                .segments
                .first()
                .map_or(0, |segment| segment.original_start),
            Err(index) => {
                let segment = &self.segments[index - 1];
                segment.original_start + (offset - segment.normalized_start)
            }
        }
    }

    fn range(&self, range: Range<usize>) -> Range<usize> {
        self.to_original(range.start)..self.to_original(range.end)
    }
}

fn extract_display_math_blocks(source: &str) -> NormalizedSource {
    let lines = source.split_inclusive('\n').collect::<Vec<_>>();
    let mut line_starts = Vec::with_capacity(lines.len() + 1);
    let mut accumulated = 0;
    for line in &lines {
        line_starts.push(accumulated);
        accumulated += line.len();
    }
    line_starts.push(accumulated);

    let mut normalized = String::with_capacity(source.len());
    let mut offsets = SourceOffsetMap::new();
    let mut display_math_blocks = Vec::new();
    let mut fenced_code = None;
    let mut line_index = 0;

    while let Some(line) = lines.get(line_index).copied() {
        if let Some(fence) = fenced_code {
            offsets.begin(normalized.len(), line_starts[line_index]);
            normalized.push_str(line);
            if closes_fenced_code(line, fence) {
                fenced_code = None;
            }
            line_index += 1;
            continue;
        }

        if let Some(fence) = starts_fenced_code(line) {
            offsets.begin(normalized.len(), line_starts[line_index]);
            normalized.push_str(line);
            fenced_code = Some(fence);
            line_index += 1;
            continue;
        }

        if let Some((next_line_index, prefix, line_ending, math_source)) =
            extract_display_math_block(&lines, line_index)
        {
            let token = format!(
                "\u{e000}zcv-display-math-{}\u{e001}",
                display_math_blocks.len()
            );
            offsets.begin(normalized.len(), line_starts[line_index]);
            normalized.push_str(prefix);
            normalized.push_str(&token);
            normalized.push_str(line_ending);
            display_math_blocks.push(DisplayMathBlock {
                token,
                source: math_source,
                range: line_starts[line_index]..line_starts[next_line_index],
            });
            line_index = next_line_index;
            continue;
        }

        offsets.begin(normalized.len(), line_starts[line_index]);
        normalized.push_str(line);
        line_index += 1;
    }

    NormalizedSource {
        text: normalized,
        math_blocks: display_math_blocks,
        offsets,
    }
}

fn extract_display_math_block<'a>(
    lines: &[&'a str],
    opening_line_index: usize,
) -> Option<(usize, &'a str, &'a str, String)> {
    let opening_line = *lines.get(opening_line_index)?;
    let (prefix, line_ending) = standalone_display_delimiter(opening_line)?;
    if line_ending.is_empty() {
        return None;
    }

    let mut source = line_ending.to_owned();
    for (offset, line) in lines[opening_line_index + 1..].iter().enumerate() {
        if standalone_display_delimiter(line).is_some() {
            return Some((opening_line_index + offset + 2, prefix, line_ending, source));
        }
        source.push_str(strip_container_prefix(line, prefix));
    }
    None
}

fn standalone_display_delimiter(line: &str) -> Option<(&str, &str)> {
    let content_end = line.trim_end_matches(['\r', '\n']).len();
    let content = &line[..content_end];
    if content.trim() != "$$" {
        return None;
    }
    let delimiter_start = content.find("$$")?;
    Some((&content[..delimiter_start], &line[content_end..]))
}

fn strip_container_prefix<'a>(line: &'a str, prefix: &str) -> &'a str {
    line.strip_prefix(prefix).unwrap_or(line)
}

fn starts_fenced_code(line: &str) -> Option<(u8, usize)> {
    let content = line.trim_start().as_bytes();
    let marker = *content.first()?;
    if marker != 0x60 && marker != b'~' {
        return None;
    }
    let length = content.iter().take_while(|byte| **byte == marker).count();
    (length >= 3).then_some((marker, length))
}

fn closes_fenced_code(line: &str, (marker, minimum_length): (u8, usize)) -> bool {
    let content = line.trim_start().as_bytes();
    let length = content.iter().take_while(|byte| **byte == marker).count();
    length >= minimum_length && content[length..].iter().all(u8::is_ascii_whitespace)
}

fn restore_display_math_blocks(blocks: &mut Vec<Block>, display_math_blocks: &[DisplayMathBlock]) {
    let mut restored = Vec::with_capacity(blocks.len());
    for Block { source, kind } in blocks.drain(..) {
        match kind {
            BlockKind::Paragraph(content) => {
                let mut paragraph = Vec::new();
                let mut after_math = false;
                for inline in content {
                    let mut remaining = inline.text.as_str();
                    while let Some((position, math)) = display_math_blocks
                        .iter()
                        .filter_map(|math| {
                            remaining.find(&math.token).map(|position| (position, math))
                        })
                        .min_by_key(|(position, _)| *position)
                    {
                        if position > 0 {
                            paragraph.push(Inline {
                                text: remaining[..position].to_owned(),
                                style: inline.style.clone(),
                            });
                        }
                        push_math_paragraph(&mut restored, &mut paragraph, after_math, &source);
                        restored.push(Block {
                            source: math.range.clone(),
                            kind: BlockKind::Math {
                                source: math.source.clone(),
                                display: true,
                            },
                        });
                        remaining = &remaining[position + math.token.len()..];
                        after_math = true;
                    }
                    if !remaining.is_empty() {
                        paragraph.push(Inline {
                            text: remaining.to_owned(),
                            style: inline.style,
                        });
                    }
                }
                if after_math {
                    push_math_paragraph(&mut restored, &mut paragraph, true, &source);
                } else {
                    restored.push(Block {
                        source,
                        kind: BlockKind::Paragraph(paragraph),
                    });
                }
            }
            BlockKind::Quote(mut children) => {
                restore_display_math_blocks(&mut children, display_math_blocks);
                restored.push(Block {
                    source,
                    kind: BlockKind::Quote(children),
                });
            }
            BlockKind::List { start, mut items } => {
                for item in &mut items {
                    restore_display_math_blocks(item, display_math_blocks);
                }
                restored.push(Block {
                    source,
                    kind: BlockKind::List { start, items },
                });
            }
            kind => restored.push(Block { source, kind }),
        }
    }
    *blocks = restored;
}

fn push_math_paragraph(
    blocks: &mut Vec<Block>,
    content: &mut Vec<Inline>,
    trim_start: bool,
    source: &Range<usize>,
) {
    if trim_start {
        while let Some(first) = content.first_mut() {
            first.text = first.text.trim_start_matches('\n').to_owned();
            if !first.text.is_empty() {
                break;
            }
            content.remove(0);
        }
    }
    while let Some(last) = content.last_mut() {
        last.text = last.text.trim_end_matches('\n').to_owned();
        if !last.text.is_empty() {
            break;
        }
        content.pop();
    }
    if !content.is_empty() {
        blocks.push(Block {
            source: source.clone(),
            kind: BlockKind::Paragraph(mem::take(content)),
        });
    }
}

fn code_language(kind: CodeBlockKind<'_>) -> Option<String> {
    match kind {
        CodeBlockKind::Indented => None,
        CodeBlockKind::Fenced(language) if language.is_empty() => None,
        CodeBlockKind::Fenced(language) => Some(language.into_string()),
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn append_inline(
    active_content: &mut Vec<Inline>,
    table: &mut Option<Table>,
    text: &str,
    style: &InlineStyle,
) {
    let target = if let Some(table) = table.as_mut()
        && let Some(cell) = table.cell.as_mut()
    {
        cell
    } else {
        active_content
    };
    if let Some(previous) = target.last_mut()
        && previous.style == *style
    {
        previous.text.push_str(text);
    } else {
        target.push(Inline {
            text: text.to_owned(),
            style: style.clone(),
        });
    }
}

fn finish_active(
    active: &mut Option<ActiveBlock>,
    active_content: &mut Vec<Inline>,
    state: &mut ParseState,
) {
    let Some(active) = active.take() else {
        return;
    };
    let content = mem::take(active_content);
    if content.is_empty() {
        return;
    }
    let (source, kind) = match active {
        ActiveBlock::Heading { level, source } => (
            source,
            BlockKind::Heading {
                level,
                content,
                slug: String::new(),
            },
        ),
        ActiveBlock::Paragraph { source } => (
            source,
            standalone_image(&content).unwrap_or(BlockKind::Paragraph(content)),
        ),
        ActiveBlock::Code { language, source } => {
            let text = content.into_iter().map(|inline| inline.text).collect();
            let kind = if language.as_deref() == Some("mermaid") {
                BlockKind::Mermaid { source: text }
            } else {
                BlockKind::Code {
                    language,
                    text,
                    highlights: None,
                }
            };
            (source, kind)
        }
    };
    state.push_block(Block { source, kind });
}

fn standalone_image(content: &[Inline]) -> Option<BlockKind> {
    let [inline] = content else {
        return None;
    };
    let source = inline.style.link.clone()?;
    if !inline.style.image {
        return None;
    }
    Some(BlockKind::Image {
        source,
        alt: inline
            .text
            .strip_prefix("图片：")
            .unwrap_or(&inline.text)
            .to_owned(),
    })
}

/// 代码块文本的行切分：去掉解析器补上的结尾换行，保留中间空行。
pub(crate) fn code_lines(text: &str) -> impl Iterator<Item = &str> {
    text.strip_suffix('\n').unwrap_or(text).split('\n')
}

#[cfg(test)]
#[path = "test/document_tests.rs"]
mod tests;
