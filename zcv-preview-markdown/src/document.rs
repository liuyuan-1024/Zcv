use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Block {
    Heading {
        level: u8,
        content: Vec<Inline>,
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
    Rule,
}

enum ActiveBlock {
    Heading(u8),
    Paragraph,
    Code(Option<String>),
}

enum Container {
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Item(Vec<Block>),
}

struct Table {
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
            Some(Container::Quote(blocks)) | Some(Container::Item(blocks)) => blocks.push(block),
            Some(Container::List { .. }) | None => self.blocks.push(block),
        }
    }

    fn finish_quote(&mut self) {
        if let Some(Container::Quote(blocks)) = self.containers.pop() {
            self.push_block(Block::Quote(blocks));
        }
    }

    fn finish_list(&mut self) {
        if let Some(Container::List { start, items }) = self.containers.pop() {
            self.push_block(Block::List { start, items });
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

    let (source, display_math_blocks) = extract_display_math_blocks(source);
    for event in Parser::new_ext(&source, options) {
        match event {
            // 元数据只描述文档，不属于 Markdown 正文。
            // 必须在解析阶段丢弃，避免其文本被后续块（尤其是第一个标题）意外收集。
            Event::Start(Tag::MetadataBlock(_)) => in_metadata_block = true,
            Event::End(TagEnd::MetadataBlock(_)) => in_metadata_block = false,
            _ if in_metadata_block => {}
            Event::Start(Tag::Heading { level, .. }) => {
                finish_active(&mut active, &mut active_content, &mut state);
                active = Some(ActiveBlock::Heading(heading_level(level)));
            }
            Event::Start(Tag::Paragraph) if state.table.is_none() => {
                if active.is_none() {
                    active = Some(ActiveBlock::Paragraph);
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                finish_active(&mut active, &mut active_content, &mut state);
                active = Some(ActiveBlock::Code(code_language(kind)));
            }
            Event::InlineMath(source) => append_inline(
                &mut active_content,
                &mut state.table,
                &source,
                &InlineStyle {
                    math: true,
                    ..style.clone()
                },
            ),
            Event::DisplayMath(source) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.push_block(Block::Math {
                    source: source.into_string(),
                    display: true,
                });
            }
            Event::Start(Tag::BlockQuote(_)) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.containers.push(Container::Quote(Vec::new()));
            }
            Event::Start(Tag::List(start)) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.containers.push(Container::List {
                    start,
                    items: Vec::new(),
                });
            }
            Event::Start(Tag::Item) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.containers.push(Container::Item(Vec::new()));
                active = Some(ActiveBlock::Paragraph);
            }
            Event::Start(Tag::Table(alignments)) => {
                finish_active(&mut active, &mut active_content, &mut state);
                state.table = Some(Table {
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
                    let row = std::mem::take(&mut table.row);
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
                        table.header = std::mem::take(&mut table.row);
                    }
                    table.in_header = false;
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some(table) = state.table.take() {
                    state.push_block(Block::Table {
                        alignments: table.alignments,
                        header: table.header,
                        rows: table.rows,
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
                state.push_block(Block::Rule);
            }
            _ => {}
        }
    }

    finish_active(&mut active, &mut active_content, &mut state);
    restore_display_math_blocks(&mut state.blocks, &display_math_blocks);
    state.blocks
}

struct DisplayMathBlock {
    token: String,
    source: String,
}

fn extract_display_math_blocks(source: &str) -> (String, Vec<DisplayMathBlock>) {
    let lines = source.split_inclusive('\n').collect::<Vec<_>>();
    let mut normalized = String::with_capacity(source.len());
    let mut display_math_blocks = Vec::new();
    let mut fenced_code = None;
    let mut line_index = 0;

    while let Some(line) = lines.get(line_index).copied() {
        if let Some(fence) = fenced_code {
            normalized.push_str(line);
            if closes_fenced_code(line, fence) {
                fenced_code = None;
            }
            line_index += 1;
            continue;
        }

        if let Some(fence) = starts_fenced_code(line) {
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
            normalized.push_str(prefix);
            normalized.push_str(&token);
            normalized.push_str(line_ending);
            display_math_blocks.push(DisplayMathBlock {
                token,
                source: math_source,
            });
            line_index = next_line_index;
            continue;
        }

        normalized.push_str(line);
        line_index += 1;
    }

    (normalized, display_math_blocks)
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
    if marker != b'`' && marker != b'~' {
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

fn restore_display_math_blocks(blocks: &mut [Block], display_math_blocks: &[DisplayMathBlock]) {
    for block in blocks {
        match block {
            Block::Paragraph(content) => {
                let Some([inline]) = content.get(..) else {
                    continue;
                };
                if inline.style != InlineStyle::default() {
                    continue;
                }
                let Some(math) = display_math_blocks
                    .iter()
                    .find(|math| math.token == inline.text)
                else {
                    continue;
                };
                *block = Block::Math {
                    source: math.source.clone(),
                    display: true,
                };
            }
            Block::Quote(children) => restore_display_math_blocks(children, display_math_blocks),
            Block::List { items, .. } => {
                for item in items {
                    restore_display_math_blocks(item, display_math_blocks);
                }
            }
            _ => {}
        }
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
    let content = std::mem::take(active_content);
    if content.is_empty() {
        return;
    }
    let block = match active {
        ActiveBlock::Heading(level) => Block::Heading { level, content },
        ActiveBlock::Paragraph => standalone_image(&content).unwrap_or(Block::Paragraph(content)),
        ActiveBlock::Code(language) => Block::Code {
            language,
            text: content.into_iter().map(|inline| inline.text).collect(),
            highlights: None,
        },
    };
    state.push_block(block);
}

fn standalone_image(content: &[Inline]) -> Option<Block> {
    let [inline] = content else {
        return None;
    };
    let source = inline.style.link.clone()?;
    if !inline.style.image {
        return None;
    }
    Some(Block::Image {
        source,
        alt: inline
            .text
            .strip_prefix("图片：")
            .unwrap_or(&inline.text)
            .to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use pulldown_cmark::Alignment;

    use super::{Block, Inline, InlineStyle, parse};

    fn plain(text: &str) -> Inline {
        Inline {
            text: text.into(),
            style: InlineStyle::default(),
        }
    }

    fn paragraph(text: &str) -> Block {
        Block::Paragraph(vec![plain(text)])
    }

    #[test]
    fn parses_common_markdown_blocks() {
        let blocks = parse(
            "# 标题\n\n正文 `code`\n\n- 一\n- 二\n\n> 引用\n\n```rust\nlet x = 1;\n```\n\n---\n",
        );
        assert_eq!(
            blocks,
            vec![
                Block::Heading {
                    level: 1,
                    content: vec![plain("标题")]
                },
                Block::Paragraph(vec![
                    plain("正文 "),
                    Inline {
                        text: "code".into(),
                        style: InlineStyle {
                            code: true,
                            ..Default::default()
                        },
                    },
                ]),
                Block::List {
                    start: None,
                    items: vec![vec![paragraph("一")], vec![paragraph("二")]],
                },
                Block::Quote(vec![paragraph("引用")]),
                Block::Code {
                    language: Some("rust".into()),
                    text: "let x = 1;\n".into(),
                    highlights: None,
                },
                Block::Rule,
            ]
        );
    }

    #[test]
    fn ignores_yaml_front_matter_before_parsing_document_blocks() {
        let blocks = parse(
            "---\nname: zcv-performance-optimization\ndescription: 性能优化\n---\n\n# Zcv 性能优化\n",
        );

        assert_eq!(
            blocks,
            vec![Block::Heading {
                level: 1,
                content: vec![plain("Zcv 性能优化")],
            }]
        );
    }

    #[test]
    fn preserves_inline_styles_links_and_tables() {
        let blocks = parse(
            "*强调* **加粗** ~~删除~~ [链接](https://zcv.dev) ![封面](cover.png)\n\n| 名称 | 值 |\n| :--- | ---: |\n| Zcv | 编辑器 |\n",
        );
        assert_eq!(
            blocks,
            vec![
                Block::Paragraph(vec![
                    Inline {
                        text: "强调".into(),
                        style: InlineStyle {
                            emphasis: true,
                            ..Default::default()
                        }
                    },
                    plain(" "),
                    Inline {
                        text: "加粗".into(),
                        style: InlineStyle {
                            strong: true,
                            ..Default::default()
                        }
                    },
                    plain(" "),
                    Inline {
                        text: "删除".into(),
                        style: InlineStyle {
                            strikethrough: true,
                            ..Default::default()
                        }
                    },
                    plain(" "),
                    Inline {
                        text: "链接".into(),
                        style: InlineStyle {
                            link: Some("https://zcv.dev".into()),
                            ..Default::default()
                        }
                    },
                    plain(" "),
                    Inline {
                        text: "图片：封面".into(),
                        style: InlineStyle {
                            link: Some("cover.png".into()),
                            image: true,
                            ..Default::default()
                        }
                    },
                ]),
                Block::Table {
                    alignments: vec![Alignment::Left, Alignment::Right],
                    header: vec![vec![plain("名称")], vec![plain("值")]],
                    rows: vec![vec![vec![plain("Zcv")], vec![plain("编辑器")]]],
                },
            ]
        );
    }

    #[test]
    fn preserves_strikethrough_and_autolink() {
        assert_eq!(
            parse("~~已废弃的描述~~ <https://zcv.dev>"),
            vec![Block::Paragraph(vec![
                Inline {
                    text: "已废弃的描述".into(),
                    style: InlineStyle {
                        strikethrough: true,
                        ..Default::default()
                    }
                },
                plain(" "),
                Inline {
                    text: "https://zcv.dev".into(),
                    style: InlineStyle {
                        link: Some("https://zcv.dev".into()),
                        ..Default::default()
                    }
                },
            ])]
        );
    }

    #[test]
    fn resolves_markdown_link_variants_and_standalone_images() {
        assert_eq!(
            parse(
                "[内联](https://zcv.dev/inline) [引用][reference] [快捷]\n\n![封面](assets/cover.png)\n\n[reference]: https://zcv.dev/reference\n[快捷]: https://zcv.dev/shortcut\n",
            ),
            vec![
                Block::Paragraph(vec![
                    Inline {
                        text: "内联".into(),
                        style: InlineStyle {
                            link: Some("https://zcv.dev/inline".into()),
                            ..Default::default()
                        }
                    },
                    plain(" "),
                    Inline {
                        text: "引用".into(),
                        style: InlineStyle {
                            link: Some("https://zcv.dev/reference".into()),
                            ..Default::default()
                        }
                    },
                    plain(" "),
                    Inline {
                        text: "快捷".into(),
                        style: InlineStyle {
                            link: Some("https://zcv.dev/shortcut".into()),
                            ..Default::default()
                        }
                    },
                ]),
                Block::Image {
                    source: "assets/cover.png".into(),
                    alt: "封面".into()
                },
            ]
        );
    }

    #[test]
    fn renders_each_source_line_break_without_creating_a_paragraph() {
        assert_eq!(
            parse("第一行\n第二行\n\n第三段"),
            vec![paragraph("第一行\n第二行"), paragraph("第三段")]
        );
    }

    #[test]
    fn does_not_treat_markdown_forced_break_syntax_as_another_line_break() {
        assert_eq!(parse("第一行  \n第二行"), vec![paragraph("第一行 第二行")]);
    }

    #[test]
    fn uses_a_checkmark_for_completed_task_list_items() {
        assert_eq!(
            parse("- [ ] 未完成\n- [x] 已完成"),
            vec![Block::List {
                start: None,
                items: vec![vec![paragraph("[ ] 未完成")], vec![paragraph("[✓] 已完成")]],
            }]
        );
    }

    #[test]
    fn parses_inline_and_display_math() {
        assert_eq!(
            parse("内联 $x^2$。\n\n$$\\frac{1}{2}$$"),
            vec![
                Block::Paragraph(vec![
                    plain("内联 "),
                    Inline {
                        text: "x^2".into(),
                        style: InlineStyle {
                            math: true,
                            ..Default::default()
                        }
                    },
                    plain("。")
                ]),
                Block::Math {
                    source: "\\frac{1}{2}".into(),
                    display: true,
                }
            ]
        );
    }

    #[test]
    fn keeps_standalone_equals_inside_display_math() {
        assert_eq!(
            parse("$$\na\n=\nb\n=\nc\n$$"),
            vec![Block::Math {
                source: "\na\n=\nb\n=\nc\n".into(),
                display: true,
            }]
        );
    }

    #[test]
    fn preserves_nested_quotes_and_lists() {
        assert_eq!(
            parse(
                "> 一级引用\n>\n> > 二级引用\n\n- 外层无序项\n  - 嵌套无序项\n    - 更深一级\n\n1. 外层有序项\n   1. 嵌套有序项\n   2. 另一项\n",
            ),
            vec![
                Block::Quote(vec![
                    paragraph("一级引用"),
                    Block::Quote(vec![paragraph("二级引用")]),
                ]),
                Block::List {
                    start: None,
                    items: vec![vec![
                        paragraph("外层无序项"),
                        Block::List {
                            start: None,
                            items: vec![vec![
                                paragraph("嵌套无序项"),
                                Block::List {
                                    start: None,
                                    items: vec![vec![paragraph("更深一级")]],
                                },
                            ]],
                        },
                    ]],
                },
                Block::List {
                    start: Some(1),
                    items: vec![vec![
                        paragraph("外层有序项"),
                        Block::List {
                            start: Some(1),
                            items: vec![vec![paragraph("嵌套有序项")], vec![paragraph("另一项")],],
                        },
                    ]],
                },
            ]
        );
    }
}
