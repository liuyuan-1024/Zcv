//! Markdown 预览渲染：CommonMark + GFM 子集 → GPUI 元素树。
//!
//! 架构：
//! - block 用 pulldown_cmark 事件栈构建；
//! - 行内内容不做 div-per-span，而是用 `StyledText::with_highlights` 附加 byte-range →
//!   `HighlightStyle` 映射，一个段落只生成一个 `StyledText`；
//! - 列表 marker（`•` / `1.`）与正文分离，正文是完整文本块；
//! - 任务列表 checkbox 渲染为 ☑/☐ 并附加颜色；
//! - 粗体 / 斜体 / 删除线 / 链接 / 行内代码通过 `compute_highlight_style` 将 style_stack 折叠为 `HighlightStyle`，支持嵌套样式叠加。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    AnyElement, FontStyle, FontWeight, HighlightStyle, Hsla, ImageSource, InteractiveText,
    ObjectFit, ScrollAnchor, ScrollHandle, SharedString, StrikethroughStyle, StyledText,
    UnderlineStyle, div, img, prelude::*, px,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::theme::syntax as syntax_theme;
use crate::theme::{color, radius, space, typography};
use zom_workspace::syntax::SyntaxEngine;

pub(super) fn render(
    source: &str,
    scroll_handle: Option<ScrollHandle>,
    syntax_engine: Option<&SyntaxEngine>,
    base_dir: Option<&Path>,
) -> impl IntoElement {
    let blocks = parse_blocks(source, syntax_engine, base_dir, scroll_handle.clone());
    let mut container = div()
        .id("markdown-preview")
        .flex_1()
        .min_w_0()
        .w_full()
        .overflow_y_scroll()
        .p(space::s16())
        .flex()
        .flex_col()
        .gap_3()
        .bg(color::current().gray.s01)
        .text_size(body_size())
        .text_color(color::current().gray.s09);
    // 仅在有持久句柄时挂 track_scroll——句柄跨帧 / 跨 tab 切换复用，
    // 内部 Rc<RefCell<..>> 自动保持滚动位置。
    if let Some(handle) = scroll_handle {
        container = container.track_scroll(&handle);
    }
    container.children(blocks)
}

/// 解析并构建顶层 block 流。
fn parse_blocks(
    source: &str,
    syntax_engine: Option<&SyntaxEngine>,
    base_dir: Option<&Path>,
    scroll_handle: Option<ScrollHandle>,
) -> Vec<AnyElement> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut builder = Builder::new(syntax_engine, base_dir, scroll_handle);
    for event in Parser::new_ext(source, options) {
        builder.handle(event);
    }
    builder.finish()
}

// ───────────────── 构建器 ─────────────────

struct Builder<'a> {
    block_stack: Vec<BlockFrame>,
    inline_stack: Vec<InlineFrame>,
    style_stack: Vec<StyleMod>,
    code_block: Option<CodeBlockBuf>,
    image_buf: Option<ImageBuf>,
    syntax_engine: Option<&'a SyntaxEngine>,
    base_dir: Option<PathBuf>,
    /// 当前行内 frame 的 kind——用于跨 image 分割后重建同类型 frame。
    current_inline_kind: Option<InlineKind>,
    /// image 分割行内流时暂存的已完结元素段。
    inline_segments: Vec<AnyElement>,
    /// 元素计数器——为每个 img 生成唯一 ElementId，用于跨帧状态持久化。
    element_counter: u64,
    /// 预览滚动句柄——锚点链接点击时用于滚动到目标标题。
    scroll_handle: Option<ScrollHandle>,
    /// 标题锚点：slug → ScrollAnchor，锚点链接点击时查找并滚动。
    heading_anchors: Rc<RefCell<HashMap<String, ScrollAnchor>>>,
}

struct CodeBlockBuf {
    lang: Option<String>,
    text: String,
}

struct ImageBuf {
    dest_url: String,
    title: String,
    alt: String,
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
    spans: Vec<StyledSpan>,
    /// 行内链接字节区间及 URL——在 `wrap_inline` 时转为 `Text::on_click`。
    link_spans: Vec<LinkSpan>,
}

/// 行内链接的（字节区间, 目标 URL）。
struct LinkSpan {
    start: usize,
    end: usize,
    url: String,
}

/// 一个带样式的文本段——同一个段内样式一致，可安全合并。
#[derive(Clone)]
struct StyledSpan {
    text: String,
    style: HighlightStyle,
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
    Link,
}

/// 将当前的 style_stack 折叠为一个 HighlightStyle——嵌套样式自然叠加。
fn compute_highlight_style(stack: &[StyleMod]) -> HighlightStyle {
    let mut style = HighlightStyle::default();
    for modifier in stack {
        match modifier {
            StyleMod::Bold => style = style.highlight(FontWeight::BOLD.into()),
            StyleMod::Italic => style = style.highlight(FontStyle::Italic.into()),
            StyleMod::Strikethrough => {
                style = style.highlight(HighlightStyle {
                    strikethrough: Some(StrikethroughStyle {
                        thickness: px(1.0),
                        color: None,
                    }),
                    ..Default::default()
                });
            }
            StyleMod::Link => {
                style = style.highlight(HighlightStyle {
                    color: Some(link_color()),
                    underline: Some(UnderlineStyle {
                        thickness: px(1.0),
                        color: None,
                        wavy: false,
                    }),
                    ..Default::default()
                });
            }
        }
    }
    style
}

/// 行内代码高亮样式。
fn code_highlight_style() -> HighlightStyle {
    HighlightStyle {
        color: Some(code_fg()),
        background_color: Some(code_bg()),
        ..Default::default()
    }
}

/// 链接颜色。
fn link_color() -> Hsla {
    color::current().blue.s07.into()
}

/// 复选框样式。
fn checkbox_style(checked: bool) -> HighlightStyle {
    HighlightStyle {
        color: Some(if checked {
            checked_color()
        } else {
            unchecked_color()
        }),
        ..Default::default()
    }
}

fn checked_color() -> Hsla {
    color::current().blue.s07.into()
}

fn unchecked_color() -> Hsla {
    color::current().gray.s06.into()
}

/// 行内代码前景。
fn code_fg() -> Hsla {
    color::current().blue.s08.into()
}

/// 行内代码背景。
fn code_bg() -> Hsla {
    color::current().gray.s02.into()
}

impl<'a> Builder<'a> {
    fn new(
        syntax_engine: Option<&'a SyntaxEngine>,
        base_dir: Option<&Path>,
        scroll_handle: Option<ScrollHandle>,
    ) -> Self {
        Self {
            block_stack: vec![BlockFrame {
                kind: BlockKind::Root,
                children: Vec::new(),
            }],
            inline_stack: Vec::new(),
            style_stack: Vec::new(),
            code_block: None,
            image_buf: None,
            syntax_engine,
            base_dir: base_dir.map(|p| p.to_path_buf()),
            current_inline_kind: None,
            inline_segments: Vec::new(),
            element_counter: 0,
            scroll_handle,
            heading_anchors: Rc::new(RefCell::new(HashMap::new())),
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
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::FootnoteReference(text) => self.on_text(text.as_ref()),
            Event::TaskListMarker(checked) => {
                let marker = if checked { "☑ " } else { "☐ " };
                self.push_inline_text_with_style(marker, checkbox_style(checked));
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.current_inline_kind = Some(InlineKind::Paragraph);
                self.inline_stack.push(InlineFrame {
                    kind: InlineKind::Paragraph,
                    spans: Vec::new(),
                    link_spans: Vec::new(),
                });
            }
            Tag::Heading { level, .. } => {
                self.current_inline_kind = Some(InlineKind::Heading(level));
                self.inline_stack.push(InlineFrame {
                    kind: InlineKind::Heading(level),
                    spans: Vec::new(),
                    link_spans: Vec::new(),
                });
            }
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
            Tag::Link { dest_url, .. } => {
                // 若链接出现在 tight list / blockquote 等缺少 Paragraph
                // 包裹的上下文中，inline_stack 可能为空——按 push_inline_text
                // 的约定自动创建 Paragraph frame。
                if self.inline_stack.is_empty() {
                    self.inline_stack.push(InlineFrame {
                        kind: InlineKind::Paragraph,
                        spans: Vec::new(),
                        link_spans: Vec::new(),
                    });
                }
                // 记录链接在当前行内 frame 全文本中的起始字节位置。
                if let Some(frame) = self.inline_stack.last_mut() {
                    let start = frame.spans.iter().map(|s| s.text.len()).sum();
                    frame.link_spans.push(LinkSpan {
                        start,
                        end: 0,
                        url: dest_url.to_string(),
                    });
                }
                self.style_stack.push(StyleMod::Link);
            }
            Tag::Image {
                dest_url, title, ..
            } => {
                self.split_inline();
                self.image_buf = Some(ImageBuf {
                    dest_url: dest_url.to_string(),
                    title: title.to_string(),
                    alt: String::new(),
                });
            }
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
            TagEnd::Paragraph | TagEnd::Heading(_) => {
                self.current_inline_kind = None;
                self.close_inline();
            }
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
                    self.push_block_element(code_block_element(
                        &buf.text,
                        buf.lang.as_deref(),
                        self.syntax_engine,
                    ));
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.style_stack.pop();
            }
            TagEnd::Link => {
                // 记录链接在当前行内 frame 全文本中的结束字节位置。
                if let Some(frame) = self.inline_stack.last_mut() {
                    let end = frame.spans.iter().map(|s| s.text.len()).sum();
                    if let Some(ls) = frame.link_spans.last_mut() {
                        ls.end = end;
                    }
                }
                self.style_stack.pop();
            }
            TagEnd::Image => {
                if let Some(buf) = self.image_buf.take() {
                    self.element_counter += 1;
                    let img_id = ("md-img", self.element_counter);
                    let element = image_element(
                        &buf.dest_url,
                        &buf.alt,
                        &buf.title,
                        self.base_dir.as_deref(),
                        img_id,
                    );
                    self.inline_segments.push(element);
                }
            }
            _ => {}
        }
    }

    fn on_text(&mut self, text: &str) {
        if let Some(buf) = self.code_block.as_mut() {
            buf.text.push_str(text);
            return;
        }

        if let Some(buf) = self.image_buf.as_mut() {
            buf.alt.push_str(text);
            return;
        }

        if text.is_empty() {
            return;
        }

        self.push_inline_text(text);
    }

    fn on_inline_code(&mut self, text: &str) {
        // 以代码样式写入：等宽字体（通过 div font 间接控制） + 底色 + 前景色。
        self.push_inline_text_with_style(text, code_highlight_style());
    }

    fn push_inline_text(&mut self, text: &str) {
        let style = compute_highlight_style(&self.style_stack);
        self.push_inline_text_with_style(text, style);
    }

    fn push_inline_text_with_style(&mut self, text: &str, style: HighlightStyle) {
        if self.inline_stack.is_empty() {
            self.inline_stack.push(InlineFrame {
                kind: InlineKind::Paragraph,
                spans: Vec::new(),
                link_spans: Vec::new(),
            });
        }

        let top = self.inline_stack.last_mut().expect("just pushed if empty");
        // 若最后一个 span 样式一致则合并，避免碎片化。
        if let Some(last) = top.spans.last_mut()
            && last.style == style
        {
            last.text.push_str(text);
        } else {
            top.spans.push(StyledSpan {
                text: text.to_string(),
                style,
            });
        }
    }

    /// 冲刷当前行内 frame，产出一个元素。若已因 image 分割而积攒了
    /// `inline_segments`，则一并将当前 frame 并入，最后用 flex_col 容器包装。
    fn close_inline(&mut self) {
        let frame = self.inline_stack.pop();
        let base_dir = self.base_dir.as_deref();
        let scroll_handle = self.scroll_handle.as_ref();
        let heading_anchors = &self.heading_anchors;

        // 无分割 → 快路径。
        if self.inline_segments.is_empty() {
            let Some(frame) = frame else { return };
            if frame.spans.is_empty() {
                return;
            }
            let is_heading = matches!(frame.kind, InlineKind::Heading(_));
            if is_heading {
                self.element_counter += 1;
            }
            let elem_id = if is_heading {
                Some(("md-h", self.element_counter))
            } else {
                None
            };
            let element = wrap_inline(frame, base_dir, scroll_handle, heading_anchors, elem_id);
            self.push_block_element(element);
            return;
        }

        // 有 image 分割 → 收尾当前 frame 并拼装。
        if let Some(frame) = frame {
            if !frame.spans.is_empty() {
                let is_heading = matches!(frame.kind, InlineKind::Heading(_));
                if is_heading {
                    self.element_counter += 1;
                }
                let elem_id = if is_heading {
                    Some(("md-h", self.element_counter))
                } else {
                    None
                };
                let element = wrap_inline(frame, base_dir, scroll_handle, heading_anchors, elem_id);
                self.inline_segments.push(element);
            }
        }

        let segments = std::mem::take(&mut self.inline_segments);
        let body: AnyElement = if segments.len() == 1 {
            segments.into_iter().next().unwrap()
        } else {
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .children(segments)
                .into_any_element()
        };

        self.push_block_element(body);
    }

    /// 在 image 边界处切断当前行内流：关闭当前 frame（若有文本则产出元素
    /// 并暂存进 `inline_segments`），随后用 `current_inline_kind` 另起一个同类型空 frame。
    fn split_inline(&mut self) {
        let frame = self.inline_stack.pop();
        let base_dir = self.base_dir.as_deref();
        let scroll_handle = self.scroll_handle.as_ref();
        let heading_anchors = &self.heading_anchors;
        if let Some(frame) = frame {
            if !frame.spans.is_empty() {
                let is_heading = matches!(frame.kind, InlineKind::Heading(_));
                if is_heading {
                    self.element_counter += 1;
                }
                let elem_id = if is_heading {
                    Some(("md-h", self.element_counter))
                } else {
                    None
                };
                let element = wrap_inline(frame, base_dir, scroll_handle, heading_anchors, elem_id);
                self.inline_segments.push(element);
            }
        }
        if let Some(kind) = self.current_inline_kind {
            self.inline_stack.push(InlineFrame {
                kind,
                spans: Vec::new(),
                link_spans: Vec::new(),
            });
        }
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
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .pl_3()
            .border_l_2()
            .border_color(color::current().gray.s04)
            .text_color(color::current().gray.s08)
            .children(frame.children)
            .into_any_element(),

        BlockKind::List { ordered_start } => {
            let mut col = div().w_full().min_w_0().flex().flex_col().gap_1().pl_4();

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
                    .text_color(color::current().gray.s07)
                    .font(typography::editor_font())
                    .child(SharedString::from(marker_text));

                // 注意这里：
                // content 只承载一个完整的 item block，而 item block 内的段落是 StyledText。
                // 不再把 "first" / "second" 等行内文本拆成多个 div 参与 flex 测量。
                let content = div().flex_1().min_w_0().child(child);

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
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .children(frame.children)
            .into_any_element(),

        BlockKind::Table => div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .py_2()
            .border_1()
            .border_color(color::current().gray.s04)
            .rounded(radius::r4())
            .children(frame.children)
            .into_any_element(),

        // pulldown_cmark 0.13 不生成 TableRow 包裹 TableHead child——TableHead
        // 里直接就是 TableCell。所以 TableHead 和 TableRow 都要做 flex_row，
        // TableCell 把 py_1 自带，row/head 不再多套一层 div。
        // 表格使用 flex_1 (basis:0%) 而非 flex_auto —— 只有各行的列都均分宽度，
        // 表头和表体才能列对齐。min_w_0 让单元格正文拿到有限宽度，交给
        // StyledText 做软换行，而不是被长内容撑开。
        BlockKind::TableHead => {
            let mut row = div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_row()
                .items_start()
                .gap_2()
                .px_2()
                .py_1()
                .font_weight(FontWeight::BOLD)
                .bg(color::current().gray.s02);
            for child in frame.children {
                row = row.child(child);
            }
            row.into_any_element()
        }

        BlockKind::TableRow => {
            let mut row = div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_row()
                .items_start()
                .gap_2()
                .px_2()
                .py_1();
            for child in frame.children {
                row = row.child(child);
            }
            row.into_any_element()
        }

        BlockKind::TableCell => div()
            .flex_1()
            .min_w_0()
            .px_1()
            .children(frame.children)
            .into_any_element(),
    }
}

/// 把 markdown 链接 URL 解析为外部可打开的 URL 字符串。
///
/// - http(s):// → 原样返回
/// - # 开头 → 锚点链接，返回 None（无法外部打开）
/// - 其余 → 相对路径，基于 base_dir 转为 file:// URL
fn resolve_link_url(raw: &str, base_dir: Option<&Path>) -> Option<String> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some(raw.to_string());
    }
    if raw.starts_with('#') {
        return None;
    }
    let path = if raw.starts_with('/') {
        PathBuf::from(raw)
    } else {
        base_dir?.join(raw)
    };
    // macOS file:// URL 格式。
    Some(format!("file://{}", path.display()))
}

/// 把标题文字转成锚点 slug：中文等非 ASCII 字符原样保留，ASCII 转小写、
/// 空格/标点替换为 `-`，连续 `-` 合并。
fn slugify(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace() || ch == '-' || ch == '_' {
            slug.push('-');
        } else if !ch.is_ascii() {
            // 非 ASCII（中文等）原样保留——GitHub 风格锚点。
            slug.push(ch);
        }
        // 其余 ASCII 标点直接丢弃。
    }
    // 合并连续 `-`。
    let mut dedup = String::with_capacity(slug.len());
    let mut prev_dash = false;
    for ch in slug.chars() {
        if ch == '-' {
            if !prev_dash {
                dedup.push('-');
            }
            prev_dash = true;
        } else {
            dedup.push(ch);
            prev_dash = false;
        }
    }
    // 去掉首尾 `-`。
    dedup.trim_matches('-').to_string()
}

fn wrap_inline(
    frame: InlineFrame,
    base_dir: Option<&Path>,
    scroll_handle: Option<&ScrollHandle>,
    heading_anchors: &Rc<RefCell<HashMap<String, ScrollAnchor>>>,
    elem_id: Option<(&'static str, u64)>,
) -> AnyElement {
    let (full_text, highlights) = build_highlighted_text(&frame.spans);
    let highlights = flatten_highlights(highlights, &full_text);
    let styled = StyledText::new(full_text).with_highlights(highlights);

    // 将 link_spans 转为 InteractiveText::on_click 的 (range, url) 对。
    let link_ranges: Vec<(std::ops::Range<usize>, String)> = frame
        .link_spans
        .iter()
        .filter(|ls| ls.end > ls.start)
        .map(|ls| (ls.start..ls.end, ls.url.clone()))
        .collect();

    let heading_anchors_for_click = Rc::clone(heading_anchors);

    let text_element: AnyElement = if link_ranges.is_empty() {
        styled.into_any_element()
    } else {
        let ranges: Vec<std::ops::Range<usize>> =
            link_ranges.iter().map(|(r, _)| r.clone()).collect();
        // 预先解析 URL：http(s) → 原样，file → file://，anchor → "#…" 标记。
        let urls: Vec<Option<String>> = link_ranges
            .iter()
            .map(|(_, raw)| resolve_link_url(raw, base_dir))
            .collect();
        let anchors: Vec<Option<String>> = link_ranges
            .iter()
            .map(|(_, raw)| {
                if raw.starts_with('#') {
                    Some(raw[1..].to_string())
                } else {
                    None
                }
            })
            .collect();
        let text_id = "md-text";
        InteractiveText::new(text_id, styled)
            .on_click(ranges, move |idx, _window, app| {
                // 锚点链接优先：滚动到目标标题。
                if let Some(Some(anchor)) = anchors.get(idx) {
                    let map = heading_anchors_for_click.borrow();
                    if let Some(scroll_anchor) = map.get(anchor) {
                        scroll_anchor.scroll_to(_window, app);
                        return;
                    }
                }
                // http(s) / file:// / 相对路径 → 全部走 resolve_link_url 处理后 open_url。
                if let Some(Some(url)) = urls.get(idx) {
                    app.open_url(url);
                }
            })
            .into_any_element()
    };

    let base = div()
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .line_height(typography::editor_line())
        .child(text_element);

    match frame.kind {
        InlineKind::Paragraph => base.into_any_element(),
        InlineKind::Heading(level) => {
            let heading_text: String = frame
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>();
            let slug = slugify(&heading_text);

            if let (Some(handle), Some(id), false) = (scroll_handle, elem_id, slug.is_empty()) {
                let anchor = ScrollAnchor::for_handle(handle.clone());
                heading_anchors.borrow_mut().insert(slug, anchor.clone());
                base.id(id)
                    .text_size(heading_size(level))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color::current().gray.s09)
                    .anchor_scroll(Some(anchor))
                    .into_any_element()
            } else {
                base.text_size(heading_size(level))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color::current().gray.s09)
                    .into_any_element()
            }
        }
    }
}

/// 将 span 列表拼接为完整文本 + byte-range → HighlightStyle 映射。
fn build_highlighted_text(
    spans: &[StyledSpan],
) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
    let mut text = String::new();
    let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();

    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        let end = text.len();
        if span.style != HighlightStyle::default() {
            highlights.push((start..end, span.style));
        }
    }

    (text, highlights)
}

fn code_block_element(
    text: &str,
    lang: Option<&str>,
    syntax_engine: Option<&SyntaxEngine>,
) -> AnyElement {
    let mut block = div()
        .p(space::s8())
        .rounded(radius::r4())
        .bg(color::current().gray.s02)
        .text_color(color::current().gray.s09)
        .font(typography::editor_font())
        .text_size(typography::editor())
        .line_height(typography::editor_line());

    if let Some(lang) = lang {
        block = block.child(
            div()
                .text_size(typography::ui())
                .text_color(color::current().gray.s07)
                .mb_1()
                .child(SharedString::from(lang.to_string())),
        );
    }

    let trimmed = text.trim_end_matches('\n');
    if trimmed.is_empty() {
        return block.into_any_element();
    }

    // 尝试语法高亮：SyntaxEngine 内聚了别名归一化 + provider 查找 + 一次性 highlight query。
    if let (Some(lang), Some(engine)) = (lang, syntax_engine) {
        if let Some(spans) = engine.highlight_snippet(lang, trimmed) {
            let highlights: Vec<_> = spans
                .into_iter()
                .map(|(range, name)| {
                    (
                        range,
                        HighlightStyle {
                            color: Some(syntax_theme::color_for(name.as_str())),
                            ..Default::default()
                        },
                    )
                })
                .collect();
            let text = trimmed.to_string();
            let highlights = flatten_highlights(highlights, &text);
            return block
                .child(StyledText::new(text).with_highlights(highlights))
                .into_any_element();
        }
    }

    // 退化路径：无语言标签 / 无引擎 / 不支持的语言 → 逐行纯文本。
    for line in trimmed.lines() {
        block = block.child(SharedString::from(line.to_string()));
    }

    block.into_any_element()
}

/// 把可能重叠的 highlight range 展开为不重叠的相邻段。
///
/// tree-sitter 对同一段文本可能产出嵌套 capture（如 `string` 里嵌套
/// `string.special`）。GPUI `compute_runs` 假设 range 不重叠且按 start 有序——
/// 重叠会导致内部字节指针倒退、TextRun 总长度错位，最终触发 text_system panic。
///
/// 展平后：重叠区域以后出现的 range 的 style 为准；相邻同 style 段自动合并。
///
/// **前置条件**：所有 range 的 start/end 必须落在 UTF-8 char boundary 上。
/// （`SyntaxEngine::highlight_snippet` 保证这一点；行内路径 range 天然满足。）
fn flatten_highlights(
    highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    text: &str,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    debug_assert!(
        highlights
            .iter()
            .all(|(r, _)| { text.is_char_boundary(r.start) && text.is_char_boundary(r.end) }),
        "flatten_highlights: 所有 range 端点必须在 char boundary 上"
    );

    if highlights.is_empty() {
        return highlights;
    }

    // 收集所有端点并排序去重
    let mut points: Vec<usize> = Vec::with_capacity(highlights.len() * 2);
    for (range, _) in &highlights {
        points.push(range.start);
        points.push(range.end);
    }
    points.sort();
    points.dedup();

    // 逐段确定 style（后出现的 range 胜出），合并相邻同 style 段
    let mut flat: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
    for window in points.windows(2) {
        let seg_start = window[0];
        let seg_end = window[1];
        let style = highlights
            .iter()
            .rev()
            .find(|(range, _)| range.start <= seg_start && range.end >= seg_end)
            .map(|(_, style)| *style);

        if let Some(style) = style {
            match flat.last_mut() {
                Some((last_range, last_style))
                    if *last_style == style && last_range.end == seg_start =>
                {
                    last_range.end = seg_end;
                }
                _ => flat.push((seg_start..seg_end, style)),
            }
        }
    }

    flat
}

/// 把 markdown 里的图片 URL 解析为 GPUI `ImageSource`。
///
/// - http(s):// → `Resource::Uri`
/// - / 开头 → 文件系统绝对路径 → `Resource::Path`
/// - 其余 → 相对于 base_dir 解析 → `Resource::Path`
fn resolve_image_source(dest_url: &str, base_dir: Option<&Path>) -> ImageSource {
    if dest_url.starts_with("http://") || dest_url.starts_with("https://") {
        return ImageSource::from(dest_url.to_string());
    }
    let path = if dest_url.starts_with('/') {
        PathBuf::from(dest_url)
    } else {
        match base_dir {
            Some(base) => base.join(dest_url),
            None => PathBuf::from(dest_url),
        }
    };
    ImageSource::from(path)
}

fn image_element(
    dest_url: &str,
    alt: &str,
    title: &str,
    base_dir: Option<&Path>,
    id: (&'static str, u64),
) -> AnyElement {
    let source = resolve_image_source(dest_url, base_dir);
    let fallback_text = if alt.is_empty() { "(图片)" } else { alt };
    let fallback = {
        let fb = fallback_text.to_string();
        move || {
            div()
                .w_full()
                .py_2()
                .px_3()
                .bg(color::current().gray.s02)
                .border_1()
                .border_color(color::current().gray.s04)
                .rounded(radius::r4())
                .text_color(color::current().gray.s07)
                .text_size(typography::ui())
                .child(SharedString::from(format!("🖼 {fb}")))
                .into_any_element()
        }
    };
    // .id(…) 将 Img 转为 Stateful<Img>，使异步加载状态能跨帧持久化。
    let img_el = img(source)
        .id(id)
        .object_fit(ObjectFit::ScaleDown)
        .w_full()
        .min_h(px(100.0))
        .max_h(px(400.0))
        .rounded(radius::r4())
        .with_fallback(fallback);
    let mut wrapper = div().w_full().min_w_0().py_2().child(img_el);
    // 有 title 时在其下方附加小字
    if !title.is_empty() {
        wrapper = wrapper.child(
            div()
                .w_full()
                .text_size(typography::ui())
                .text_color(color::current().gray.s06)
                .mt_1()
                .child(SharedString::from(title.to_string())),
        );
    }
    wrapper.into_any_element()
}

fn horizontal_rule() -> AnyElement {
    div()
        .h(px(1.0))
        .w_full()
        .flex_none()
        .my_2()
        .bg(color::current().gray.s04)
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
        parse_blocks(source, None, None, None).len()
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

    // ─── 行内样式 / GFM 烟测 ─────────────────────────────────────────────────────

    #[test]
    fn bold_text_renders_one_block() {
        assert_eq!(block_count("**bold** and normal"), 1);
    }

    #[test]
    fn italic_text_renders_one_block() {
        assert_eq!(block_count("*italic* and normal"), 1);
    }

    #[test]
    fn strikethrough_renders_one_block() {
        assert_eq!(block_count("~~strikethrough~~ normal"), 1);
    }

    #[test]
    fn link_renders_one_block() {
        assert_eq!(block_count("[link](https://example.com)"), 1);
    }

    #[test]
    fn inline_code_renders_one_block() {
        assert_eq!(block_count("use `std::io` in code"), 1);
    }

    #[test]
    fn bold_italic_nested_renders_one_block() {
        assert_eq!(block_count("***bold italic*** normal"), 1);
    }

    #[test]
    fn task_list_checkbox_renders_one_block() {
        assert_eq!(block_count("- [x] done\n- [ ] pending\n"), 1);
    }

    #[test]
    fn image_alt_text_renders_one_block() {
        assert_eq!(block_count("![alt](img.png)"), 1);
    }

    #[test]
    fn thematic_break_renders() {
        // --- / *** / ___ 各自单独成行
        assert_eq!(block_count("---\n"), 1);
        assert_eq!(block_count("***\n"), 1);
        assert_eq!(block_count("___\n"), 1);
        // 带空格的写法
        assert_eq!(block_count("- - -\n"), 1);
    }

    #[test]
    fn flatten_resolves_overlapping_ranges_last_wins() {
        let text = "abcdefg";
        let style_a = HighlightStyle {
            color: Some(Hsla::default()),
            ..Default::default()
        };
        let style_b = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        // 0..6 (style_a) 与 3..7 (style_b) 在 3..6 重叠
        let highlights = vec![(0..6, style_a), (3..7, style_b)];
        let flat = flatten_highlights(highlights, text);
        // 0..3 → style_a; 3..7 → style_b（重叠区后出现的胜出）
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].0, 0..3);
        assert_eq!(flat[0].1, style_a);
        assert_eq!(flat[1].0, 3..7);
        assert_eq!(flat[1].1, style_b);
    }

    #[test]
    fn flatten_merges_adjacent_same_style() {
        let text = "abcdef";
        let style = HighlightStyle {
            color: Some(Hsla::default()),
            ..Default::default()
        };
        // 两个相邻不重叠的同 style range → 应合并
        let highlights = vec![(0..3, style), (3..6, style)];
        let flat = flatten_highlights(highlights, text);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].0, 0..6);
        assert_eq!(flat[0].1, style);
    }

    #[test]
    fn flatten_empty_input_is_noop() {
        let flat = flatten_highlights(Vec::new(), "anything");
        assert!(flat.is_empty());
    }
}
