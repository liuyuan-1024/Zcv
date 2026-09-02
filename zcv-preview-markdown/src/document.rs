use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Block {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph(String),
    Code(String),
    Quote(String),
    List {
        start: Option<u64>,
        items: Vec<String>,
    },
    Rule,
}

enum ActiveBlock {
    Heading(u8),
    Paragraph,
    Code,
    Quote,
}

struct List {
    start: Option<u64>,
    items: Vec<String>,
}

pub(crate) fn parse(source: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut active = None;
    let mut active_text = String::new();
    let mut list = None;
    let mut list_item = None;

    for event in Parser::new_ext(source, Options::all()) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                finish_active(&mut active, &mut active_text, &mut list_item, &mut blocks);
                active = Some(ActiveBlock::Heading(heading_level(level)));
            }
            Event::Start(Tag::Paragraph) => {
                if !matches!(active, Some(ActiveBlock::Quote)) {
                    finish_active(&mut active, &mut active_text, &mut list_item, &mut blocks);
                    active = Some(ActiveBlock::Paragraph);
                }
            }
            Event::Start(Tag::CodeBlock(_)) => {
                finish_active(&mut active, &mut active_text, &mut list_item, &mut blocks);
                active = Some(ActiveBlock::Code);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                finish_active(&mut active, &mut active_text, &mut list_item, &mut blocks);
                active = Some(ActiveBlock::Quote);
            }
            Event::Start(Tag::List(start)) => {
                finish_active(&mut active, &mut active_text, &mut list_item, &mut blocks);
                list = Some(List {
                    start,
                    items: Vec::new(),
                });
            }
            Event::Start(Tag::Item) => {
                list_item = Some(String::new());
            }
            Event::End(TagEnd::Heading(_))
            | Event::End(TagEnd::CodeBlock)
            | Event::End(TagEnd::BlockQuote(_)) => {
                finish_active(&mut active, &mut active_text, &mut list_item, &mut blocks);
            }
            Event::End(TagEnd::Paragraph) if matches!(active, Some(ActiveBlock::Quote)) => {}
            Event::End(TagEnd::Paragraph) => {
                finish_active(&mut active, &mut active_text, &mut list_item, &mut blocks);
            }
            Event::End(TagEnd::Item) => {
                finish_active(&mut active, &mut active_text, &mut list_item, &mut blocks);
                if let (Some(list), Some(item)) = (list.as_mut(), list_item.take()) {
                    list.items.push(item.trim().to_owned());
                }
            }
            Event::End(TagEnd::List(_)) => {
                finish_active(&mut active, &mut active_text, &mut list_item, &mut blocks);
                if let Some(list) = list.take() {
                    blocks.push(Block::List {
                        start: list.start,
                        items: list.items,
                    });
                }
            }
            Event::Text(text) | Event::Code(text) | Event::Html(text) | Event::InlineHtml(text) => {
                append_text(&mut active_text, &mut list_item, &text);
            }
            Event::SoftBreak | Event::HardBreak => {
                append_text(&mut active_text, &mut list_item, "\n")
            }
            Event::TaskListMarker(done) => {
                append_text(
                    &mut active_text,
                    &mut list_item,
                    if done { "[x] " } else { "[ ] " },
                );
            }
            Event::Rule => {
                finish_active(&mut active, &mut active_text, &mut list_item, &mut blocks);
                blocks.push(Block::Rule);
            }
            _ => {}
        }
    }

    finish_active(&mut active, &mut active_text, &mut list_item, &mut blocks);
    if let Some(list) = list {
        blocks.push(Block::List {
            start: list.start,
            items: list.items,
        });
    }
    blocks
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

fn append_text(active_text: &mut String, list_item: &mut Option<String>, text: &str) {
    if let Some(item) = list_item {
        item.push_str(text);
    } else {
        active_text.push_str(text);
    }
}

fn finish_active(
    active: &mut Option<ActiveBlock>,
    active_text: &mut String,
    list_item: &mut Option<String>,
    blocks: &mut Vec<Block>,
) {
    let Some(active) = active.take() else {
        return;
    };
    let text = std::mem::take(active_text);
    if let Some(item) = list_item {
        item.push_str(&text);
        return;
    }
    let block = match active {
        ActiveBlock::Heading(level) => Block::Heading { level, text },
        ActiveBlock::Paragraph => Block::Paragraph(text),
        ActiveBlock::Code => Block::Code(text),
        ActiveBlock::Quote => Block::Quote(text),
    };
    blocks.push(block);
}

#[cfg(test)]
mod tests {
    use super::{Block, parse};

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
                    text: "标题".into(),
                },
                Block::Paragraph("正文 code".into()),
                Block::List {
                    start: None,
                    items: vec!["一".into(), "二".into()],
                },
                Block::Quote("引用".into()),
                Block::Code("let x = 1;\n".into()),
                Block::Rule,
            ]
        );
    }
}
