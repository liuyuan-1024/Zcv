//! Markdown 块级与行内构建器 —— 将 pulldown_cmark 事件流转为元素。
//!
//! Builder 维护 block_stack / inline_stack / style_stack 三栈状态机，在事件流结束时产出 GPUI 元素树。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    AnyElement, FontStyle, FontWeight, HighlightStyle, Hsla, ScrollAnchor, ScrollHandle,
    StrikethroughStyle, UnderlineStyle, prelude::*, px,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Tag, TagEnd};

use crate::theme::color;
use zom_workspace::syntax::SyntaxEngine;

use super::elements::{
    code_block_element, horizontal_rule, image_element, wrap_block, wrap_inline,
    wrap_inline_as_flex_items,
};
use super::math;

// ───────────────── 行内元素段 ─────────────────

/// 行内流中暂存的元素段，最终在 close_inline 中组装为 flex_row + flex_wrap。
pub(super) enum InlineSegment {
    /// 文本字词——在 flex 容器中用 flex_none 包裹。
    Text(AnyElement),
    /// 公式/图片等媒体——直接以自然尺寸参与 flex 流。
    Media(AnyElement),
    /// flex_wrap 强制断行——w_full + 0 高度，把后续元素挤到下一 flex 行。
    Break,
}

// ───────────────── 构建器 ─────────────────

pub(super) struct Builder<'a> {
    pub(super) block_stack: Vec<BlockFrame>,
    pub(super) inline_stack: Vec<InlineFrame>,
    pub(super) style_stack: Vec<StyleMod>,
    pub(super) code_block: Option<CodeBlockBuf>,
    pub(super) image_buf: Option<ImageBuf>,
    pub(super) syntax_engine: Option<&'a SyntaxEngine>,
    pub(super) base_dir: Option<PathBuf>,
    /// 当前行内 frame 的 kind——用于跨 image 分割后重建同类型 frame。
    pub(super) current_inline_kind: Option<InlineKind>,
    /// image/公式/换行 分割行内流时暂存的已完结元素段。
    pub(super) inline_segments: Vec<InlineSegment>,
    /// 元素计数器——为每个 img 生成唯一 ElementId，用于跨帧状态持久化。
    pub(super) element_counter: u64,
    /// 预览滚动句柄——锚点链接点击时用于滚动到目标标题。
    pub(super) scroll_handle: Option<ScrollHandle>,
    /// 标题锚点：slug → ScrollAnchor，锚点链接点击时查找并滚动。
    pub(super) heading_anchors: Rc<RefCell<HashMap<String, ScrollAnchor>>>,
}

pub(super) struct CodeBlockBuf {
    pub(super) lang: Option<String>,
    pub(super) text: String,
}

pub(super) struct ImageBuf {
    pub(super) dest_url: String,
    pub(super) title: String,
    pub(super) alt: String,
}

pub(super) struct BlockFrame {
    pub(super) kind: BlockKind,
    pub(super) children: Vec<AnyElement>,
}

pub(super) enum BlockKind {
    Root,
    BlockQuote,
    List { ordered_start: Option<u64> },
    Item,
    Table,
    TableHead,
    TableRow,
    TableCell,
}

pub(super) struct InlineFrame {
    pub(super) kind: InlineKind,
    pub(super) spans: Vec<StyledSpan>,
    /// 行内链接字节区间及 URL——在 `wrap_inline` 时转为 `InteractiveText::on_click`。
    pub(super) link_spans: Vec<LinkSpan>,
}

/// 行内链接的（字节区间, 目标 URL）。
pub(super) struct LinkSpan {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) url: String,
}

/// 一个带样式的文本段——同一个段内样式一致，可安全合并。
#[derive(Clone)]
pub(super) struct StyledSpan {
    pub(super) text: String,
    pub(super) style: HighlightStyle,
}

#[derive(Clone, Copy)]
pub(super) enum InlineKind {
    Paragraph,
    Heading(HeadingLevel),
}

#[derive(Clone)]
pub(super) enum StyleMod {
    Bold,
    Italic,
    Strikethrough,
    Link,
}

// ───────────────── 样式折叠与颜色 ─────────────────

/// 将当前的 style_stack 折叠为一个 HighlightStyle——嵌套样式自然叠加。
pub(super) fn compute_highlight_style(stack: &[StyleMod]) -> HighlightStyle {
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
pub(super) fn code_highlight_style() -> HighlightStyle {
    HighlightStyle {
        color: Some(code_fg()),
        background_color: Some(code_bg()),
        ..Default::default()
    }
}

/// 链接颜色。
pub(super) fn link_color() -> Hsla {
    color::current().blue.s07.into()
}

/// 复选框样式。
pub(super) fn checkbox_style(checked: bool) -> HighlightStyle {
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

// ───────────────── Builder 实现 ─────────────────

impl<'a> Builder<'a> {
    pub(super) fn new(
        syntax_engine: Option<&'a SyntaxEngine>,
        base_dir: Option<&std::path::Path>,
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

    pub(super) fn finish(mut self) -> Vec<AnyElement> {
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

    pub(super) fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.on_text(text.as_ref()),
            Event::Code(text) => self.on_inline_code(text.as_ref()),
            Event::InlineMath(text) => {
                // 行内公式：拆分行内流，作为自然宽度媒体嵌入文本流。
                self.split_inline();
                let element = math::math_element(text.as_ref(), false);
                self.inline_segments.push(InlineSegment::Media(element));
            }
            Event::DisplayMath(text) => {
                // 块级公式：$$...$$ → 居中、自然尺寸。
                self.close_inline();
                let element = gpui::div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(math::math_element(text.as_ref(), true))
                    .into_any_element();
                self.push_block_element(element);
            }
            Event::SoftBreak | Event::HardBreak => {
                // 软换行（单回车）和硬换行统一处理：在 flex 流中插入强制断行。
                // w_full + 0 高度让 flex_wrap 把后续内容挤到下一行，
                // 行间距由 line_height 自然控制。
                self.split_inline();
                self.inline_segments.push(InlineSegment::Break);
            }
            Event::Rule => self.push_block_element(horizontal_rule()),
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::FootnoteReference(text) => self.on_text(text.as_ref()),
            Event::TaskListMarker(checked) => {
                let marker = if checked { "☑ " } else { "☐ " };
                self.push_inline_text_with_style(marker, checkbox_style(checked));
            }
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
            // 这些块级标签在 tight list 中出现在没有 Paragraph 包裹的上下文——文本直接作为 Text 事件流入 inline_stack。
            // 在进入块级元素之前冲刷 inline_stack，否则文案会在 End(Item) 才冲刷，导致渲染顺序块在上、文字在下。
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
                // 若链接出现在 tight list / blockquote 等缺少 Paragraph 包裹的上下文中，inline_stack 可能为空——按 push_inline_text 的约定自动创建 Paragraph frame。
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
            // Item / TableCell 需要先冲刷 inline_stack：
            // （tight list 中 pulldown_cmark 省略了 Paragraph 事件，
            // 文本直接从 item 流入 Text→push_inline_text）不先冲刷的话，
            // End(Item) pop block 时 children 是空的，文字会泄漏到 Root。
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
                    self.inline_segments.push(InlineSegment::Media(element));
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

    /// 冲刷当前行内 frame，产出一个元素。
    /// 若已因 image 分割而积攒了 `inline_segments`，则一并将当前 frame 并入，最后用 flex_col 容器包装。
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

        // 有分割 → 收尾当前 frame 并拼装。
        if let Some(frame) = frame
            && !frame.spans.is_empty()
        {
            for el in wrap_inline_as_flex_items(frame, base_dir, scroll_handle, heading_anchors) {
                self.inline_segments.push(InlineSegment::Text(el));
            }
        }

        let segments = std::mem::take(&mut self.inline_segments);
        let body: AnyElement = if segments.len() == 1 {
            match segments.into_iter().next().unwrap() {
                InlineSegment::Text(el) | InlineSegment::Media(el) => el,
                InlineSegment::Break => return, // 孤立断行无意义
            }
        } else {
            // 多段混排（文本字词 + 公式/图片 + 换行）→ flex_row + flex_wrap。
            let children: Vec<AnyElement> = segments
                .into_iter()
                .map(|seg| match seg {
                    InlineSegment::Text(el) => gpui::div().flex_none().child(el).into_any_element(),
                    InlineSegment::Media(el) => el,
                    InlineSegment::Break => gpui::div()
                        .w_full()
                        .h(px(0.0))
                        .flex_none()
                        .into_any_element(),
                })
                .collect();

            gpui::div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .children(children)
                .into_any_element()
        };

        self.push_block_element(body);
    }

    /// 在 image/公式 边界处切断当前行内流：
    /// 关闭当前 frame（若有文本则产出元素并暂存进 `inline_segments`），随后用 `current_inline_kind` 另起一个同类型空 frame。
    fn split_inline(&mut self) {
        let frame = self.inline_stack.pop();
        let base_dir = self.base_dir.as_deref();
        let scroll_handle = self.scroll_handle.as_ref();
        let heading_anchors = &self.heading_anchors;
        if let Some(frame) = frame
            && !frame.spans.is_empty()
        {
            // 逐词拆分为多个 flex_none 子元素，让 flex_wrap 在字词边界自然换行。
            for el in wrap_inline_as_flex_items(frame, base_dir, scroll_handle, heading_anchors) {
                self.inline_segments.push(InlineSegment::Text(el));
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
