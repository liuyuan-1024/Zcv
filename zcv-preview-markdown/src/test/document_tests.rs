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
    let blocks =
        parse("# 标题\n\n正文 `code`\n\n- 一\n- 二\n\n> 引用\n\n```rust\nlet x = 1;\n```\n\n---\n");
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
