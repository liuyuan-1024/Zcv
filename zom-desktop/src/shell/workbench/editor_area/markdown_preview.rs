//! Markdown 预览渲染：CommonMark + GFM 子集 → GPUI 元素树。
//!
//! Zed-style 最小重写版：
//! - block 仍然用 pulldown_cmark 事件栈构建；
//! - 行内内容不再渲染成一堆 `div().child(SharedString)`；
//! - 每个段落 / 标题只生成一个 `StyledText` 文本元素；
//! - 列表 marker 和列表正文分离，正文是完整文本块，避免 GPUI flex 测量时只显示 `•`、正文被压成 0 宽。
//!
//! 说明：
//! 这一版优先修复列表文字不可见问题。粗体 / 斜体 / 链接 / 行内代码会先保留为纯文本内容；
//! 如果后续要恢复精细行内样式，应该像 Zed 的 markdown crate 一样给 `StyledText` 附加 TextRun，
//! 而不是重新把每个 span 拆成 div。

use gpui::{AnyElement, FontWeight, SharedString, StyledText, div, prelude::*, px};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::theme::{color, radius, space, typography};

pub(super) fn render(source: &str) -> impl IntoElement {
    let blocks = parse_blocks(source);
    div()
        .id("markdown-preview")
        .flex_1()
        .overflow_y_scroll()
        .p(space::s16())
        .flex()
        .flex_col()
        .gap_3()
        .bg(color::gray::s01())
        .text_size(body_size())
        .text_color(color::gray::s09())
        .children(blocks)
}

/// 解析并构建顶层 block 流。
fn parse_blocks(source: &str) -> Vec<AnyElement> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut builder = Builder::new();
    for event in Parser::new_ext(source, options) {
        builder.handle(event);
    }
    builder.finish()
}

// ───────────────── 构建器 ─────────────────

struct Builder {
    block_stack: Vec<BlockFrame>,
    inline_stack: Vec<InlineFrame>,
    style_stack: Vec<StyleMod>,
    code_block: Option<CodeBlockBuf>,
}

struct CodeBlockBuf {
    lang: Option<String>,
    text: String,
}

struct BlockFrame {
    kind: BlockKind,
    children: Vec<AnyElement>,
}

enum BlockKind {
    Root,
    BlockQuote,
    List { ordered_start: Option<u64> },
    Item,
    Table,
    TableHead,
    TableRow,
    TableCell,
}

struct InlineFrame {
    kind: InlineKind,
    text: String,
}

#[derive(Clone, Copy)]
enum InlineKind {
    Paragraph,
    Heading(HeadingLevel),
}

#[derive(Clone)]
enum StyleMod {
    Bold,
    Italic,
    Strikethrough,
    Link(SharedString),
    Image(SharedString),
}

impl Builder {
    fn new() -> Self {
        Self {
            block_stack: vec![BlockFrame {
                kind: BlockKind::Root,
                children: Vec::new(),
            }],
            inline_stack: Vec::new(),
            style_stack: Vec::new(),
            code_block: None,
        }
    }

    fn finish(mut self) -> Vec<AnyElement> {
        while !self.inline_stack.is_empty() {
            self.close_inline();
        }

        while self.block_stack.len() > 1 {
            let frame = self.block_stack.pop().expect("non-root frame exists");
            let element = wrap_block(frame);
            self.push_block_element(element);
        }

        self.block_stack.pop().expect("root frame").children
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.on_text(text.as_ref()),
            Event::Code(text) => self.on_inline_code(text.as_ref()),
            Event::SoftBreak => self.on_text(" "),
            Event::HardBreak => self.on_text("\n"),
            Event::Rule => self.push_block_element(horizontal_rule()),
            Event::Html(text) | Event::InlineHtml(text) => self.on_text(text.as_ref()),
            Event::FootnoteReference(text) => self.on_text(text.as_ref()),
            Event::TaskListMarker(checked) => {
                let marker = if checked { "[x] " } else { "[ ] " };
                self.on_text(marker);
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.inline_stack.push(InlineFrame {
                kind: InlineKind::Paragraph,
                text: String::new(),
            }),
            Tag::Heading { level, .. } => self.inline_stack.push(InlineFrame {
                kind: InlineKind::Heading(level),
                text: String::new(),
            }),
            // 这些块级标签在 tight list 中出现在里没有 Paragraph 包裹——文本直接作为
            // Text 事件流入 inline_stack。在进入块级元素之前冲刷 inline_stack，
            // 否则文案会在 End(Item) 才冲刷，导致渲染顺序块在上、文字在下。
            Tag::BlockQuote(_) | Tag::List(_) | Tag::CodeBlock(_) | Tag::Table(_) => {
                self.close_inline();
                match tag {
                    Tag::BlockQuote(_) => self.block_stack.push(BlockFrame {
                        kind: BlockKind::BlockQuote,
                        children: Vec::new(),
                    }),
                    Tag::List(start) => self.block_stack.push(BlockFrame {
                        kind: BlockKind::List {
                            ordered_start: start,
                        },
                        children: Vec::new(),
                    }),
                    Tag::CodeBlock(kind) => {
                        let lang = match kind {
                            CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                                Some(lang.to_string())
                            }
                            _ => None,
                        };
                        self.code_block = Some(CodeBlockBuf {
                            lang,
                            text: String::new(),
                        });
                    }
                    Tag::Table(_) => self.block_stack.push(BlockFrame {
                        kind: BlockKind::Table,
                        children: Vec::new(),
                    }),
                    _ => unreachable!(),
                }
            }
            Tag::Item => self.block_stack.push(BlockFrame {
                kind: BlockKind::Item,
                children: Vec::new(),
            }),
            Tag::Emphasis => self.style_stack.push(StyleMod::Italic),
            Tag::Strong => self.style_stack.push(StyleMod::Bold),
            Tag::Strikethrough => self.style_stack.push(StyleMod::Strikethrough),
            Tag::Link { dest_url, .. } => self
                .style_stack
                .push(StyleMod::Link(SharedString::from(dest_url.into_string()))),
            Tag::Image { dest_url, .. } => self
                .style_stack
                .push(StyleMod::Image(SharedString::from(dest_url.into_string()))),
            Tag::TableHead => self.block_stack.push(BlockFrame {
                kind: BlockKind::TableHead,
                children: Vec::new(),
            }),
            Tag::TableRow => self.block_stack.push(BlockFrame {
                kind: BlockKind::TableRow,
                children: Vec::new(),
            }),
            Tag::TableCell => self.block_stack.push(BlockFrame {
                kind: BlockKind::TableCell,
                children: Vec::new(),
            }),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => self.close_inline(),
            // Item / TableCell 需要先冲刷 inline_stack：（tight list 中 pulldown_cmark 省略了
            // Paragraph 事件，文本直接从 item 流入 Text→push_inline_text）
            // 不先冲刷的话，End(Item) pop block 时 children 是空的，文字会泄漏到 Root。
            TagEnd::Item | TagEnd::TableCell => {
                self.close_inline();
                let frame = self.block_stack.pop().expect("matching open frame");
                let element = wrap_block(frame);
                self.push_block_element(element);
            }
            TagEnd::BlockQuote(_)
            | TagEnd::List(_)
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow => {
                let frame = self.block_stack.pop().expect("matching open frame");
                let element = wrap_block(frame);
                self.push_block_element(element);
            }
            TagEnd::CodeBlock => {
                if let Some(buf) = self.code_block.take() {
                    self.push_block_element(code_block_element(&buf.text, buf.lang.as_deref()));
                }
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image => {
                self.style_stack.pop();
            }
            _ => {}
        }
    }

    fn on_text(&mut self, text: &str) {
        if let Some(buf) = self.code_block.as_mut() {
            buf.text.push_str(text);
            return;
        }

        if text.is_empty() {
            return;
        }

        let mut text = text.to_string();

        // 图片不再尝试行内渲染成独立 div，只把 alt 文本合并进当前 StyledText。
        if self
            .style_stack
            .iter()
            .any(|modifier| matches!(modifier, StyleMod::Image(_)))
        {
            text = format!("🖼 {text}");
        }

        self.push_inline_text(&text);
    }

    fn on_inline_code(&mut self, text: &str) {
        // 先保留为普通文本。后续要恢复代码样式，应在 StyledText 上加 TextRun，
        // 不要重新拆成多个 div。
        self.push_inline_text(text);
    }

    fn push_inline_text(&mut self, text: &str) {
        if self.inline_stack.is_empty() {
            self.inline_stack.push(InlineFrame {
                kind: InlineKind::Paragraph,
                text: String::new(),
            });
        }

        let top = self.inline_stack.last_mut().expect("just pushed if empty");
        top.text.push_str(text);
    }

    fn close_inline(&mut self) {
        let Some(frame) = self.inline_stack.pop() else {
            return;
        };

        if frame.text.is_empty() {
            return;
        }

        let element = wrap_inline(frame);
        self.push_block_element(element);
    }

    fn push_block_element(&mut self, element: AnyElement) {
        self.block_stack
            .last_mut()
            .expect("root frame always present")
            .children
            .push(element);
    }
}

// ───────────────── 块构造 ─────────────────

fn wrap_block(frame: BlockFrame) -> AnyElement {
    match frame.kind {
        BlockKind::Root => unreachable!("Root 不进入 wrap_block"),
        BlockKind::BlockQuote => div()
            .flex()
            .flex_col()
            .gap_2()
            .pl_3()
            .border_l_2()
            .border_color(color::gray::s04())
            .text_color(color::gray::s08())
            .children(frame.children)
            .into_any_element(),

        BlockKind::List { ordered_start } => {
            let mut col = div().w_full().flex().flex_col().gap_1().pl_4();

            let mut index = ordered_start.unwrap_or(1);

            for child in frame.children {
                let marker_text = match ordered_start {
                    Some(_) => {
                        let label = format!("{index}.");
                        index += 1;
                        label
                    }
                    None => "•".to_string(),
                };

                let marker = div()
                    .w(px(28.0))
                    .flex_none()
                    .text_color(color::gray::s07())
                    .font(typography::editor_font())
                    .child(SharedString::from(marker_text));

                // 注意这里：
                // content 只承载一个完整的 item block，而 item block 内的段落是 StyledText。
                // 不再把 "first" / "second" 等行内文本拆成多个 div 参与 flex 测量。
                let content = div().flex_auto().w_full().child(child);

                let row = div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_1()
                    .child(marker)
                    .child(content);

                col = col.child(row);
            }

            col.into_any_element()
        }

        BlockKind::Item => div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .children(frame.children)
            .into_any_element(),

        BlockKind::Table => div()
            .flex()
            .flex_col()
            .gap_1()
            .py_2()
            .border_1()
            .border_color(color::gray::s04())
            .rounded(radius::r4())
            .children(frame.children)
            .into_any_element(),

        // pulldown_cmark 0.13 不生成 TableRow 包裹 TableHead child——TableHead
        // 里直接就是 TableCell。所以 TableHead 和 TableRow 都要做 flex_row，
        // TableCell 把 py_1 自带，row/head 不再多套一层 div。
        // 表格使用 flex_1 (basis:0%) 而非 flex_auto —— 只有各行的列都均分宽度，
        // 表头和表体才能列对齐。去掉 min_w(0) 后，taffy 默认的 min-width:auto
        // 防止内容塌陷到 0，所以 flex_1 在这里是安全的。
        BlockKind::TableHead => {
            let mut row = div()
                .flex()
                .flex_row()
                .items_start()
                .gap_2()
                .px_2()
                .py_1()
                .font_weight(FontWeight::BOLD)
                .bg(color::gray::s02());
            for child in frame.children {
                row = row.child(child);
            }
            row.into_any_element()
        }

        BlockKind::TableRow => {
            let mut row = div().flex().flex_row().items_start().gap_2().px_2().py_1();
            for child in frame.children {
                row = row.child(child);
            }
            row.into_any_element()
        }

        BlockKind::TableCell => div()
            .flex_1()
            .px_1()
            .children(frame.children)
            .into_any_element(),
    }
}

fn wrap_inline(frame: InlineFrame) -> AnyElement {
    let text = StyledText::new(frame.text);

    let base = div().w_full().child(text);

    match frame.kind {
        InlineKind::Paragraph => base.into_any_element(),
        InlineKind::Heading(level) => base
            .text_size(heading_size(level))
            .font_weight(FontWeight::BOLD)
            .text_color(color::gray::s09())
            .into_any_element(),
    }
}

fn code_block_element(text: &str, lang: Option<&str>) -> AnyElement {
    let mut block = div()
        .p(space::s8())
        .rounded(radius::r4())
        .bg(color::gray::s02())
        .text_color(color::gray::s09())
        .font(typography::editor_font())
        .text_size(typography::editor())
        .line_height(typography::editor_line());

    if let Some(lang) = lang {
        block = block.child(
            div()
                .text_size(typography::ui())
                .text_color(color::gray::s07())
                .mb_1()
                .child(SharedString::from(lang.to_string())),
        );
    }

    let trimmed = text.trim_end_matches('\n');
    if trimmed.is_empty() {
        return block.into_any_element();
    }

    // 代码块可以保留逐行 SharedString；它不是列表行内文本，不会触发 bullet-only 问题。
    for line in trimmed.lines() {
        block = block.child(SharedString::from(line.to_string()));
    }

    block.into_any_element()
}

fn horizontal_rule() -> AnyElement {
    div()
        .h(px(1.0))
        .w_full()
        .bg(color::gray::s04())
        .into_any_element()
}

// ───────────────── 视觉常量 ─────────────────

fn body_size() -> gpui::Pixels {
    typography::editor()
}

fn heading_size(level: HeadingLevel) -> gpui::Pixels {
    match level {
        HeadingLevel::H1 => px(26.0),
        HeadingLevel::H2 => px(22.0),
        HeadingLevel::H3 => px(19.0),
        HeadingLevel::H4 => px(17.0),
        HeadingLevel::H5 | HeadingLevel::H6 => px(16.0),
    }
}

// ─── 烟测 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn block_count(source: &str) -> usize {
        parse_blocks(source).len()
    }

    #[test]
    fn empty_input_yields_no_blocks() {
        assert_eq!(block_count(""), 0);
    }

    #[test]
    fn unordered_list_renders_as_one_top_level_block() {
        let source = "- first\n- second\n- third\n";
        assert_eq!(block_count(source), 1);
    }

    #[test]
    fn ordered_list_renders_as_one_top_level_block() {
        let source = "1. first\n2. second\n3. third\n";
        assert_eq!(block_count(source), 1);
    }

    #[test]
    fn nested_mixed_lists_are_not_truncated() {
        let source = "- first\n- second\n  1. nested one\n  2. nested two\n- third\n";
        assert_eq!(block_count(source), 1);
    }

    #[test]
    fn code_block_with_lang_renders() {
        let source = "```rust\nlet x = 1;\n```\n";
        assert_eq!(block_count(source), 1);
    }

    #[test]
    fn table_renders() {
        let source = "| a | b |\n| --- | --- |\n| c | d |\n";
        assert_eq!(block_count(source), 1);
    }

    #[test]
    fn tight_list_with_code_block_order() {
        let source = "- 无序子项中包含：\n    ```js\n    const insideList = true;\n    ```\n";
        assert_eq!(block_count(source), 1);
    }
}
