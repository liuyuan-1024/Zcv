use pulldown_cmark::Alignment;

use super::{Block, BlockKind, Inline, InlineStyle, parse as parse_blocks};

/// 解析结果去掉 source 区间后的可比较形态。
#[derive(Debug, PartialEq)]
enum T {
    Heading {
        level: u8,
        content: Vec<Inline>,
        slug: String,
    },
    Paragraph(Vec<Inline>),
    Code {
        language: Option<String>,
        text: String,
        highlights: Option<zcv_language::SnippetHighlights>,
    },
    Quote(Vec<T>),
    List {
        start: Option<u64>,
        items: Vec<Vec<T>>,
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

fn parse(source: &str) -> Vec<T> {
    parse_blocks(source).iter().map(T::from_block).collect()
}

impl T {
    fn from_block(block: &Block) -> Self {
        match &block.kind {
            BlockKind::Heading {
                level,
                content,
                slug,
            } => T::Heading {
                level: *level,
                content: content.clone(),
                slug: slug.clone(),
            },
            BlockKind::Paragraph(content) => T::Paragraph(content.clone()),
            BlockKind::Code {
                language,
                text,
                highlights,
            } => T::Code {
                language: language.clone(),
                text: text.clone(),
                highlights: highlights.clone(),
            },
            BlockKind::Quote(children) => T::Quote(children.iter().map(T::from_block).collect()),
            BlockKind::List { start, items } => T::List {
                start: *start,
                items: items
                    .iter()
                    .map(|item| item.iter().map(T::from_block).collect())
                    .collect(),
            },
            BlockKind::Table {
                alignments,
                header,
                rows,
            } => T::Table {
                alignments: alignments.clone(),
                header: header.clone(),
                rows: rows.clone(),
            },
            BlockKind::Image { source, alt } => T::Image {
                source: source.clone(),
                alt: alt.clone(),
            },
            BlockKind::Math { source, display } => T::Math {
                source: source.clone(),
                display: *display,
            },
            BlockKind::Mermaid { source } => T::Mermaid {
                source: source.clone(),
            },
            BlockKind::Rule => T::Rule,
        }
    }
}

fn plain(text: &str) -> Inline {
    Inline {
        text: text.into(),
        style: InlineStyle::default(),
    }
}

fn paragraph(text: &str) -> T {
    T::Paragraph(vec![plain(text)])
}

#[test]
fn block_source_ranges_survive_display_math_normalization() {
    // 多行 $$ 块会被压缩为单行占位，后续块必须仍映射回原始源码偏移。
    let source = "前置段落\n\n$$\na\n=\nb\n$$\n\n# 目标标题\n\n正文\n";
    let blocks = parse_blocks(source);
    let heading = blocks
        .iter()
        .find(|block| matches!(block.kind, BlockKind::Heading { .. }))
        .expect("应解析出标题");
    let expected = source.find("# 目标标题").expect("源文档应包含标题");
    assert_eq!(heading.source.start, expected);

    let math = blocks
        .iter()
        .find(|block| matches!(block.kind, BlockKind::Math { .. }))
        .expect("应解析出公式块");
    let math_start = source.find("$$").expect("源文档应包含公式");
    assert_eq!(math.source.start, math_start);
}

#[test]
fn heading_source_range_starts_at_heading_markup() {
    let source = "正文\n\n## 小节标题\n";
    let blocks = parse_blocks(source);
    let heading = blocks
        .iter()
        .find(|block| matches!(block.kind, BlockKind::Heading { .. }))
        .expect("应解析出标题");
    assert_eq!(heading.source.start, source.find("## 小节标题").unwrap());
}

#[test]
fn parses_common_markdown_blocks() {
    let blocks =
        parse("# 标题\n\n正文 `code`\n\n- 一\n- 二\n\n> 引用\n\n```rust\nlet x = 1;\n```\n\n---\n");
    assert_eq!(
        blocks,
        vec![
            T::Heading {
                level: 1,
                content: vec![plain("标题")],
                slug: "标题".into(),
            },
            T::Paragraph(vec![
                plain("正文 "),
                Inline {
                    text: "code".into(),
                    style: InlineStyle {
                        code: true,
                        ..Default::default()
                    },
                },
            ]),
            T::List {
                start: None,
                items: vec![vec![paragraph("一")], vec![paragraph("二")]],
            },
            T::Quote(vec![paragraph("引用")]),
            T::Code {
                language: Some("rust".into()),
                text: "let x = 1;\n".into(),
                highlights: None,
            },
            T::Rule,
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
        vec![T::Heading {
            level: 1,
            content: vec![plain("Zcv 性能优化")],
            slug: "zcv-性能优化".into(),
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
            T::Paragraph(vec![
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
            T::Table {
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
        vec![T::Paragraph(vec![
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
            T::Paragraph(vec![
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
            T::Image {
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
        vec![T::List {
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
            T::Paragraph(vec![
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
            T::Math {
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
        vec![T::Math {
            source: "\na\n=\nb\n=\nc\n".into(),
            display: true,
        }]
    );
}

#[test]
fn separates_display_math_from_adjacent_paragraph_lines() {
    assert_eq!(
        parse("公式如下：\n$$\na^*=\\arg\\max_a I(a)\n$$\n其中 I(a) 为信息增益。"),
        vec![
            paragraph("公式如下："),
            T::Math {
                source: "\na^*=\\arg\\max_a I(a)\n".into(),
                display: true,
            },
            paragraph("其中 I(a) 为信息增益。"),
        ]
    );
}

#[test]
fn preserves_nested_quotes_and_lists() {
    assert_eq!(
        parse(
            "> 一级引用\n>\n> > 二级引用\n\n- 外层无序项\n  - 嵌套无序项\n    - 更深一级\n\n1. 外层有序项\n   1. 嵌套有序项\n   2. 另一项\n",
        ),
        vec![
            T::Quote(vec![
                paragraph("一级引用"),
                T::Quote(vec![paragraph("二级引用")]),
            ]),
            T::List {
                start: None,
                items: vec![vec![
                    paragraph("外层无序项"),
                    T::List {
                        start: None,
                        items: vec![vec![
                            paragraph("嵌套无序项"),
                            T::List {
                                start: None,
                                items: vec![vec![paragraph("更深一级")]],
                            },
                        ]],
                    },
                ]],
            },
            T::List {
                start: Some(1),
                items: vec![vec![
                    paragraph("外层有序项"),
                    T::List {
                        start: Some(1),
                        items: vec![vec![paragraph("嵌套有序项")], vec![paragraph("另一项")],],
                    },
                ]],
            },
        ]
    );
}

#[test]
fn parses_mermaid_fence_as_diagram_and_keeps_other_code() {
    assert_eq!(
        parse("```mermaid\ngraph TD;\nA --> B;\n```\n\n```rust\nfn main() {}\n```"),
        vec![
            T::Mermaid {
                source: "graph TD;\nA --> B;\n".into(),
            },
            T::Code {
                language: Some("rust".into()),
                text: "fn main() {}\n".into(),
                highlights: None,
            },
        ]
    );
}
