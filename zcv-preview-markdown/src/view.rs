//! Markdown 预览 Item：观察源码 MultiBuffer，解析后渲染为原生块元素。

use std::any::TypeId;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, AnyEntity, App, Bounds, Context, Element, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, FontStyle, FontWeight, GlobalElementId, HighlightStyle, Hsla, Image,
    ImageFormat, InspectorElementId, InteractiveText, LayoutId, ObjectFit, Pixels, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, StrikethroughStyle, StyledImage,
    StyledText, Subscription, Task, TextAlign, TextLayout, UnderlineStyle, Window, div, fill, img,
    point, prelude::*, px,
};
use pulldown_cmark::Alignment;
use zcv_language::{
    HighlightSpan, LanguageRegistry, SnippetHighlightCancellation, SnippetHighlights,
    highlight_snippet_with_cancellation,
};
use zcv_multi_buffer::{MultiBuffer, MultiBufferEvent};
use zcv_project::Project;
use zcv_theme::{color, space, syntax, typography};
use zcv_ui::Scrollbar;
use zcv_workspace::{
    Item, ItemEvent, ItemHandle, OpenPathCallback, PreviewDocument, PreviewItem, PreviewItemHandle,
    typography_for_window,
};

use crate::document::{Block, Inline, parse};

const MARKDOWN_REPARSE_DEBOUNCE: Duration = Duration::from_millis(200);
const INLINE_CODE_CHIP_HORIZONTAL_OUTSET: Pixels = space::S2;
const INLINE_CODE_CHIP_VERTICAL_INSET: f32 = 0.1;
const INLINE_CODE_CHIP_CORNER_RADIUS: Pixels = px(4.);

struct MarkdownRenderContext<'a> {
    source_directory: Option<&'a Path>,
    open_path: Option<&'a OpenPathCallback>,
    type_scale: typography::Typography,
    math_images: &'a HashMap<String, Result<Arc<gpui::RenderImage>, String>>,
    cx: &'a App,
}

pub(crate) struct MarkdownPreviewView {
    source_item: Box<dyn ItemHandle>,
    multi_buffer: Entity<MultiBuffer>,
    /// 代码围栏高亮使用的语言注册表；与源码 MultiBuffer 共享同一份。
    language_registry: Arc<LanguageRegistry>,
    open_path: Option<OpenPathCallback>,
    focus: FocusHandle,
    scroll_handle: ScrollHandle,
    scrollbar: Scrollbar<ScrollHandle>,
    blocks: Arc<Vec<Block>>,
    code_highlight_generation: u64,
    code_highlight_cancellation: Option<SnippetHighlightCancellation>,
    code_highlight_task: Option<Task<()>>,
    refresh_generation: u64,
    refresh_task: Option<Task<()>>,
    _document_subscription: Subscription,
    _item_subscription: Subscription,
    math_images: Arc<HashMap<String, Result<Arc<gpui::RenderImage>, String>>>,
    math_content_size: Option<gpui::Pixels>,
    math_color: Option<gpui::Rgba>,
    math_render_task: Option<Task<()>>,
    math_render_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkdownPreviewEvent {
    SourceMetadataChanged,
}

impl MarkdownPreviewView {
    pub(crate) fn new(document: PreviewDocument, cx: &mut Context<Self>) -> Self {
        let PreviewDocument::Source {
            source_item,
            multi_buffer,
            open_path,
            ..
        } = document
        else {
            panic!("Markdown 预览必须从源码 Item 创建")
        };
        let document_subscription = cx.subscribe(&multi_buffer, |view, _, event, cx| {
            if matches!(event, MultiBufferEvent::TextChanged) {
                view.schedule_refresh(cx);
            }
        });
        let this = cx.entity().downgrade();
        let item_subscription = source_item.subscribe_to_item_events(
            cx,
            Box::new(move |event, cx| {
                if matches!(
                    event,
                    ItemEvent::PathChanged | ItemEvent::UpdateTab | ItemEvent::UpdateBreadcrumbs
                ) {
                    this.update(cx, |_view, cx| {
                        cx.emit(MarkdownPreviewEvent::SourceMetadataChanged);
                        cx.notify();
                    })
                    .ok();
                }
            }),
        );
        let scroll_handle = ScrollHandle::new();
        let language_registry = multi_buffer
            .read(cx)
            .language_registry(cx)
            .expect("源码派生 Markdown 预览要求 MultiBuffer 携带语言注册表；缺少说明装配顺序错误");
        let mut view = Self {
            source_item,
            multi_buffer,
            language_registry,
            open_path,
            focus: cx.focus_handle(),
            scrollbar: Scrollbar::vertical(scroll_handle.clone()),
            scroll_handle,
            blocks: Arc::new(Vec::new()),
            code_highlight_generation: 0,
            code_highlight_cancellation: None,
            code_highlight_task: None,
            refresh_generation: 0,
            refresh_task: None,
            _document_subscription: document_subscription,
            _item_subscription: item_subscription,
            math_images: Arc::new(HashMap::new()),
            math_content_size: None,
            math_color: None,
            math_render_task: None,
            math_render_generation: 0,
        };
        view.refresh(cx);
        view
    }

    fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        // 连续输入时仅保留最新一次更新，避免每个按键都重建整个预览。
        self.refresh_generation = self.refresh_generation.wrapping_add(1);
        let generation = self.refresh_generation;
        let timer = cx.background_executor().timer(MARKDOWN_REPARSE_DEBOUNCE);
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |view, cx| {
                if view.refresh_generation != generation {
                    return;
                }
                view.refresh_task = None;
                view.refresh(cx);
            });
        }));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(cancellation) = self.code_highlight_cancellation.take() {
            cancellation.cancel();
        }
        // 预览渲染需要整份文档文本；这是只读边界，不进入编辑/显示热路径。
        let text = String::from_utf8(
            self.multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx).text_bytes()),
        )
        .expect("编辑器文档应为 UTF-8");
        self.blocks = Arc::new(parse(&text));
        self.math_render_generation = self.math_render_generation.wrapping_add(1);
        self.math_content_size = None;
        self.math_color = None;
        self.math_images = Arc::new(HashMap::new());
        self.math_render_task.take();
        self.code_highlight_generation = self.code_highlight_generation.wrapping_add(1);
        let generation = self.code_highlight_generation;
        let cancellation = SnippetHighlightCancellation::default();
        self.code_highlight_cancellation = Some(cancellation.clone());
        let mut blocks = (*self.blocks).clone();
        let language_registry = Arc::clone(&self.language_registry);
        let highlights = cx.background_spawn(async move {
            highlight_code_blocks(&mut blocks, &language_registry, &cancellation).then_some(blocks)
        });
        self.code_highlight_task = Some(cx.spawn(async move |this, cx| {
            let Some(blocks) = highlights.await else {
                return;
            };
            let _ = this.update(cx, |view, cx| {
                if view.code_highlight_generation != generation {
                    return;
                }
                view.blocks = Arc::new(blocks);
                view.code_highlight_cancellation = None;
                view.code_highlight_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn ensure_math_render(
        &mut self,
        content_size: gpui::Pixels,
        math_color: gpui::Rgba,
        cx: &mut Context<Self>,
    ) {
        if self.math_content_size == Some(content_size) && self.math_color == Some(math_color) {
            return;
        }

        self.math_content_size = Some(content_size);
        self.math_color = Some(math_color);
        self.math_render_generation = self.math_render_generation.wrapping_add(1);
        let generation = self.math_render_generation;
        let math_sources = collect_math_sources(&self.blocks);
        if math_sources.is_empty() {
            self.math_images = Arc::new(HashMap::new());
            self.math_render_task = None;
            return;
        }

        self.math_render_task.take();
        let math_font_size = f64::from(content_size.as_f32());
        let math_color =
            ratex_types::color::Color::new(math_color.r, math_color.g, math_color.b, math_color.a);
        let renderer = cx.svg_renderer();
        let math_task = cx.background_spawn(async move {
            math_sources
                .into_iter()
                .map(|(source, display)| {
                    let result = render_math(
                        &source,
                        display,
                        math_font_size,
                        math_color,
                        renderer.clone(),
                    );
                    (source, result)
                })
                .collect::<HashMap<_, _>>()
        });
        self.math_render_task = Some(cx.spawn(async move |this, cx| {
            let images = math_task.await;
            let _ = this.update(cx, |view, cx| {
                if view.math_render_generation != generation {
                    return;
                }
                view.math_images = Arc::new(images);
                view.math_render_task = None;
                cx.notify();
            });
        }));
    }
}

impl EventEmitter<MarkdownPreviewEvent> for MarkdownPreviewView {}

impl Focusable for MarkdownPreviewView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MarkdownPreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let type_scale = typography_for_window(window, cx);
        self.ensure_math_render(type_scale.content_size(), color::current(cx).text, cx);
        let source_path = self.source_item.item_path(cx);
        let source_directory = source_path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let render_context = MarkdownRenderContext {
            source_directory: source_directory.as_deref(),
            open_path: self.open_path.as_ref(),
            type_scale,
            math_images: &self.math_images,
            cx,
        };
        let content = self
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| {
                let mut next_key = 0;
                div()
                    .child(render_block(
                        block,
                        &mut next_key,
                        0,
                        index,
                        &render_context,
                    ))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        div()
            .id("markdown-preview")
            .track_focus(&self.focus)
            .key_context("MarkdownPreview")
            .tab_index(0)
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(color::current(cx).editor_background)
            .child(
                div()
                    .id("markdown-preview-scroll-container")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .p(space::S6)
                    .text_color(color::current(cx).text)
                    .text_size(type_scale.content_size())
                    .line_height(type_scale.content_line())
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(space::S16)
                            .children(content),
                    ),
            )
            .child(div().absolute().inset_0().child(self.scrollbar.clone()))
    }
}

fn render_block(
    block: &Block,
    next_key: &mut usize,
    list_depth: usize,
    namespace: usize,
    render_context: &MarkdownRenderContext<'_>,
) -> AnyElement {
    let source_directory = render_context.source_directory;
    let type_scale = render_context.type_scale;
    let math_images = render_context.math_images;
    let cx = render_context.cx;
    let key = *next_key;
    *next_key += 1;
    match block {
        Block::Heading { level, content } => {
            let size = heading_size(*level, type_scale);
            div()
                .text_size(size)
                .line_height(heading_line_height(*level, size, type_scale))
                .font_weight(FontWeight::BOLD)
                .child(render_inline(content, key, render_context))
                .into_any_element()
        }
        Block::Paragraph(content) => div()
            .whitespace_normal()
            .child(render_inline(content, key, render_context))
            .into_any_element(),
        Block::Code {
            language,
            text,
            highlights,
        } => {
            let mut code = div()
                .rounded_md()
                .bg(color::current(cx).panel_background)
                .p(space::S12)
                .font(typography::content_font())
                .text_size(type_scale.content_size())
                .flex()
                .flex_col();
            if let Some(language) = language {
                code = code.child(
                    div()
                        .mb(space::S8)
                        .text_size(type_scale.content_size() * 0.85)
                        .text_color(color::current(cx).text_muted)
                        .child(language.clone()),
                );
            }
            let styles = highlights
                .as_ref()
                .map(|highlights| syntax::style_table(&highlights.capture_names, cx));
            let mut line_start = 0;
            let mut highlight_index = 0;
            for line in code_lines(text) {
                let line_end = line_start + line.len();
                code = code.child(div().child(render_code_line(
                    line,
                    line_start..line_end,
                    highlights.as_ref(),
                    styles.as_deref(),
                    &mut highlight_index,
                )));
                line_start = line_end + 1;
            }
            code.into_any_element()
        }
        Block::Quote(blocks) => {
            let children = blocks
                .iter()
                .map(|block| render_block(block, next_key, list_depth, namespace, render_context))
                .collect::<Vec<_>>();
            div()
                .border_l_2()
                .border_color(color::current(cx).border_variant)
                .pl(space::S12)
                .flex()
                .flex_col()
                .gap(space::S16)
                .text_color(color::current(cx).text_muted)
                .children(children)
                .into_any_element()
        }
        Block::List { start, items } => {
            let marker_width = list_marker_width(*start, items.len(), type_scale);
            let children = items
                .iter()
                .enumerate()
                .map(|(item_index, item)| {
                    let marker_text = start.map_or_else(
                        || "•".to_owned(),
                        |start| format!("{}.", start + item_index as u64),
                    );
                    let mut item_children = item
                        .iter()
                        .map(|block| {
                            render_block(block, next_key, list_depth + 1, namespace, render_context)
                        })
                        .collect::<Vec<_>>()
                        .into_iter();
                    let marker = || {
                        div()
                            .w(marker_width)
                            .flex_none()
                            .flex()
                            .items_center()
                            .text_left()
                            .text_color(color::current(cx).text_muted)
                            .child(marker_text.clone())
                    };
                    let mut content = div().flex().flex_col().gap(space::S4);
                    if let Some(first_child) = item_children.next() {
                        content = content.child(
                            div()
                                .flex()
                                .gap(space::S2)
                                .line_height(type_scale.content_line())
                                .child(marker())
                                .child(div().flex_1().min_w_0().child(first_child)),
                        );
                    } else {
                        content = content.child(marker());
                    }
                    content.children(item_children)
                })
                .collect::<Vec<_>>();
            div()
                .when(list_depth > 0, |list| list.pl(space::S16))
                .flex()
                .flex_col()
                .gap(space::S4)
                .children(children)
                .into_any_element()
        }
        Block::Table {
            alignments,
            header,
            rows,
        } => {
            let mut table = div()
                .id(ElementId::named_usize(
                    format!("markdown-table-{namespace}"),
                    key,
                ))
                .w_full()
                .overflow_x_scroll()
                .rounded_md()
                .border_1()
                .border_color(color::current(cx).border_variant)
                .flex()
                .flex_col();
            if !header.is_empty() {
                table = table.child(render_table_row(
                    header,
                    alignments,
                    true,
                    next_key,
                    render_context,
                ));
            }
            for row in rows {
                table = table.child(render_table_row(
                    row,
                    alignments,
                    false,
                    next_key,
                    render_context,
                ));
            }
            table.into_any_element()
        }
        Block::Image { source, alt } => render_image(source, alt, source_directory, cx),
        Block::Math { source, display } => {
            render_math_block(source, *display, key, math_images, cx)
        }
        Block::Rule => div()
            .h(px(1.))
            .w_full()
            .bg(color::current(cx).border_variant)
            .into_any_element(),
    }
}

fn list_marker_width(
    start: Option<u64>,
    item_count: usize,
    type_scale: typography::Typography,
) -> gpui::Pixels {
    let marker_char_count = list_marker_char_count(start, item_count);
    // 标记列按字符数预留，正文与编号之间只保留布局间距。
    type_scale.content_size() * (marker_char_count as f32 * 0.6)
}

fn list_marker_char_count(start: Option<u64>, item_count: usize) -> usize {
    start.map_or(1, |start| {
        let last_marker = start.saturating_add(item_count.saturating_sub(1) as u64);
        last_marker.to_string().len() + 1
    })
}

fn render_image(source: &str, alt: &str, source_directory: Option<&Path>, cx: &App) -> AnyElement {
    let fallback_alt = alt.to_owned();
    let muted = color::current(cx).text_muted;
    let loading_muted = muted;
    let image = if source.starts_with("http://") || source.starts_with("https://") {
        img(source.to_owned())
    } else {
        let path = Path::new(source);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            source_directory.map_or_else(|| path.to_path_buf(), |directory| directory.join(path))
        };
        img(path)
    }
    .max_w_full()
    .object_fit(ObjectFit::Contain)
    .with_loading(move || {
        div()
            .text_color(loading_muted)
            .child("正在加载图片…")
            .into_any_element()
    })
    .with_fallback(move || {
        div()
            .text_color(muted)
            .child(format!("无法加载图片：{fallback_alt}"))
            .into_any_element()
    });
    div()
        .w_full()
        .flex()
        .justify_center()
        .flex_none()
        .child(image)
        .into_any_element()
}

fn render_table_row(
    cells: &[Vec<Inline>],
    alignments: &[Alignment],
    is_header: bool,
    next_key: &mut usize,
    render_context: &MarkdownRenderContext<'_>,
) -> AnyElement {
    let cx = render_context.cx;
    div()
        .flex()
        .w_full()
        .when(!is_header, |row| {
            row.border_t_1()
                .border_color(color::current(cx).border_variant)
        })
        .children(cells.iter().enumerate().map(|(cell_index, cell_content)| {
            let cell = div()
                .flex_1()
                .min_w(space::S32)
                .p(space::S2)
                .when(cell_index > 0, |cell| {
                    cell.border_l_1()
                        .border_color(color::current(cx).border_variant)
                })
                .when(is_header, |cell| cell.font_weight(FontWeight::SEMIBOLD));
            let cell = match alignments
                .get(cell_index)
                .copied()
                .unwrap_or(Alignment::None)
            {
                Alignment::Left | Alignment::None => cell,
                Alignment::Center => cell.text_center(),
                Alignment::Right => cell.text_right(),
            };
            let key = *next_key;
            *next_key += 1;
            cell.child(render_inline(cell_content, key, render_context))
        }))
        .into_any_element()
}

fn render_inline(
    content: &[Inline],
    key: usize,
    render_context: &MarkdownRenderContext<'_>,
) -> AnyElement {
    let type_scale = render_context.type_scale;
    let math_images = render_context.math_images;
    let cx = render_context.cx;
    if content.iter().any(|inline| inline.style.math) {
        let mut lines = vec![Vec::new()];
        for inline in content {
            let parts = inline.text.split('\n').collect::<Vec<_>>();
            for (index, part) in parts.iter().enumerate() {
                if !part.is_empty() {
                    lines.last_mut().unwrap().push(Inline {
                        text: (*part).to_owned(),
                        style: inline.style.clone(),
                    });
                }
                if index + 1 < parts.len() {
                    lines.push(Vec::new());
                }
            }
        }
        return div()
            .flex()
            .flex_col()
            .children(lines.into_iter().enumerate().map(|(line_index, line)| {
                div()
                    .w_full()
                    .flex_wrap()
                    .min_h(type_scale.content_line())
                    .flex()
                    .items_center()
                    .children(line.iter().enumerate().map(|(index, inline)| {
                        if inline.style.math {
                            render_math_inline(&inline.text, math_images, cx)
                        } else {
                            render_text_inline(
                                std::slice::from_ref(inline),
                                key + line_index + index,
                                render_context.source_directory,
                                render_context.open_path,
                                cx,
                            )
                        }
                    }))
            }))
            .into_any_element();
    }
    render_text_inline(
        content,
        key,
        render_context.source_directory,
        render_context.open_path,
        cx,
    )
}

fn render_text_inline(
    content: &[Inline],
    key: usize,
    source_directory: Option<&Path>,
    open_path: Option<&OpenPathCallback>,
    cx: &App,
) -> AnyElement {
    let mut text = String::new();
    let mut highlights = Vec::new();
    let mut links = Vec::new();
    let mut link_ranges = Vec::new();
    let mut code_ranges = Vec::new();

    for inline in content {
        let start = text.len();
        text.push_str(&inline.text);
        let end = text.len();
        if start == end {
            continue;
        }
        let style = &inline.style;
        if style.code {
            code_ranges.push(start..end);
        }
        if style.emphasis || style.strong || style.strikethrough || style.link.is_some() {
            highlights.push((
                start..end,
                HighlightStyle {
                    font_style: style.emphasis.then_some(FontStyle::Italic),
                    font_weight: style.strong.then_some(FontWeight::BOLD),
                    strikethrough: style.strikethrough.then_some(StrikethroughStyle {
                        thickness: px(2.),
                        color: Some(color::current(cx).text.into()),
                    }),
                    color: style
                        .link
                        .as_ref()
                        .map(|_| color::current(cx).icon_accent.into()),
                    underline: style.link.as_ref().map(|_| UnderlineStyle {
                        thickness: px(2.),
                        color: Some(color::current(cx).icon_accent.into()),
                        wavy: false,
                    }),
                    ..Default::default()
                },
            ));
        }
        if let Some(url) = &style.link {
            link_ranges.push(start..end);
            links.push(url.clone());
        }
    }

    let text = StyledText::new(text).with_highlights(highlights);
    let layout = text.layout().clone();
    let text = if links.is_empty() {
        text.into_any_element()
    } else {
        let source_directory = source_directory.map(Path::to_path_buf);
        let open_path = open_path.cloned();
        InteractiveText::new(("markdown-link", key), text)
            .on_click(link_ranges, move |index, window, cx| {
                let link = &links[index];
                if let Some(path) = resolve_markdown_file_link(link, source_directory.as_deref())
                    && let Some(open_path) = &open_path
                {
                    open_path(path, window, cx);
                } else {
                    cx.open_url(link);
                }
            })
            .into_any_element()
    };
    if code_ranges.is_empty() {
        text
    } else {
        MarkdownInlineText {
            text,
            layout,
            code_ranges,
            code_color: color::current(cx).border_variant.into(),
        }
        .into_any_element()
    }
}

fn resolve_markdown_file_link(link: &str, source_directory: Option<&Path>) -> Option<PathBuf> {
    let (link, _) = link.split_once('#').unwrap_or((link, ""));
    if link.is_empty() || link.contains("://") || link.starts_with("mailto:") {
        return None;
    }
    let source_directory = source_directory?;
    let path = Path::new(link);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        source_directory.join(path)
    })
}

/// 在文本布局上绘制行内代码背景，不把视觉留白写入文本内容或交互索引。
struct MarkdownInlineText {
    text: AnyElement,
    layout: TextLayout,
    code_ranges: Vec<Range<usize>>,
    code_color: Hsla,
}

impl Element for MarkdownInlineText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.text.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.text.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        paint_inline_code_chips(&self.layout, &self.code_ranges, self.code_color, window);
        self.text.paint(window, cx);
    }
}

impl gpui::IntoElement for MarkdownInlineText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

fn paint_inline_code_chips(
    layout: &TextLayout,
    code_ranges: &[Range<usize>],
    color: Hsla,
    window: &mut Window,
) {
    let line_bounds = layout.bounds();
    let line_height = layout.line_height();
    let text_align = window.text_style().text_align;
    let mut row_top = line_bounds.origin.y;
    let mut line_start = 0;

    for line in layout.line_layouts() {
        let line_end = line_start + line.len();
        let unwrapped_layout = &line.unwrapped_layout;
        let mut row_start = line_start;
        let mut row_start_x = Pixels::ZERO;
        let row_ends = line
            .wrap_boundaries()
            .iter()
            .map(|boundary| {
                let glyph = &unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix];
                (line_start + glyph.index, glyph.position.x)
            })
            .chain([(line_end, unwrapped_layout.width)]);

        for (row_end, row_end_x) in row_ends {
            for code_range in code_ranges {
                let selection_start = code_range.start.max(row_start);
                let selection_end = code_range.end.min(row_end);
                if selection_start >= selection_end {
                    continue;
                }

                let alignment_offset = match text_align {
                    TextAlign::Left => Pixels::ZERO,
                    TextAlign::Center => {
                        ((line_bounds.size.width - (row_end_x - row_start_x)) / 2.).max(px(0.))
                    }
                    TextAlign::Right => {
                        (line_bounds.size.width - (row_end_x - row_start_x)).max(px(0.))
                    }
                };
                let x_for_index = |index| {
                    line_bounds.left()
                        + alignment_offset
                        + unwrapped_layout.x_for_index(index - line_start)
                        - row_start_x
                };
                let top = row_top + line_height * INLINE_CODE_CHIP_VERTICAL_INSET;
                let bottom = row_top + line_height * (1. - INLINE_CODE_CHIP_VERTICAL_INSET);
                window.paint_quad(
                    fill(
                        Bounds::from_corners(
                            point(
                                x_for_index(selection_start) - INLINE_CODE_CHIP_HORIZONTAL_OUTSET,
                                top,
                            ),
                            point(
                                x_for_index(selection_end) + INLINE_CODE_CHIP_HORIZONTAL_OUTSET,
                                bottom,
                            ),
                        ),
                        color,
                    )
                    .corner_radii(INLINE_CODE_CHIP_CORNER_RADIUS),
                );
            }

            row_start = row_end;
            row_start_x = row_end_x;
            row_top += line_height;
        }

        line_start = line_end + 1;
    }
}

fn render_math_inline(
    source: &str,
    math_images: &HashMap<String, Result<Arc<gpui::RenderImage>, String>>,
    cx: &App,
) -> AnyElement {
    match math_images.get(source) {
        Some(Ok(image)) => img(image.clone())
            .object_fit(ObjectFit::Contain)
            .max_w_full()
            .flex_none()
            .into_any_element(),
        Some(Err(error)) => div()
            .text_color(color::current(cx).status_error)
            .child(format!("公式渲染失败：{error}"))
            .into_any_element(),
        None => div()
            .text_color(color::current(cx).text_muted)
            .child("正在渲染公式…")
            .into_any_element(),
    }
}

fn render_math_block(
    source: &str,
    display: bool,
    key: usize,
    math_images: &HashMap<String, Result<Arc<gpui::RenderImage>, String>>,
    cx: &App,
) -> AnyElement {
    div()
        .id(("markdown-math-block", key))
        .w_full()
        .flex()
        .overflow_x_scroll()
        .when(display, |element| element.justify_center())
        .child(render_math_inline(source, math_images, cx))
        .into_any_element()
}

fn collect_math_sources(blocks: &[Block]) -> HashMap<String, bool> {
    let mut sources = HashMap::new();
    fn collect_inline(inlines: &[Inline], sources: &mut HashMap<String, bool>) {
        for inline in inlines {
            if inline.style.math {
                sources.entry(inline.text.clone()).or_insert(false);
            }
        }
    }
    fn visit(blocks: &[Block], sources: &mut HashMap<String, bool>) {
        for block in blocks {
            match block {
                Block::Math { source, display } => {
                    sources.entry(source.clone()).or_insert(*display);
                }
                Block::Heading { content, .. } | Block::Paragraph(content) => {
                    collect_inline(content, sources);
                }
                Block::Table { header, rows, .. } => {
                    for cell in header {
                        collect_inline(cell, sources);
                    }
                    for row in rows {
                        for cell in row {
                            collect_inline(cell, sources);
                        }
                    }
                }
                Block::Quote(children) => visit(children, sources),
                Block::List { items, .. } => {
                    for item in items {
                        visit(item, sources);
                    }
                }
                _ => {}
            }
        }
    }
    visit(blocks, &mut sources);
    sources
}

fn render_math(
    source: &str,
    _display: bool,
    font_size: f64,
    color: ratex_types::color::Color,
    renderer: gpui::SvgRenderer,
) -> Result<Arc<gpui::RenderImage>, String> {
    let nodes = ratex_parser::parse(source).map_err(|error| error.to_string())?;
    let layout_options = ratex_layout::LayoutOptions {
        color,
        ..Default::default()
    };
    let layout = ratex_layout::layout(&nodes, &layout_options);
    let list = ratex_layout::to_display_list(&layout);
    let svg = ratex_svg::render_to_svg(
        &list,
        &ratex_svg::SvgOptions {
            font_size,
            padding: 0.0,
            embed_glyphs: true,
            ..Default::default()
        },
    );
    Image::from_bytes(ImageFormat::Svg, svg.into_bytes())
        .to_image_data(renderer)
        .map_err(|error| error.to_string())
}

fn highlight_code_blocks(
    blocks: &mut [Block],
    language_registry: &Arc<LanguageRegistry>,
    cancellation: &SnippetHighlightCancellation,
) -> bool {
    for block in blocks {
        if cancellation.is_cancelled() {
            return false;
        }
        match block {
            Block::Code {
                language: Some(language),
                text,
                highlights,
            } => {
                *highlights = highlight_snippet_with_cancellation(
                    language_registry,
                    language,
                    text,
                    cancellation,
                )
            }
            Block::Quote(blocks) => {
                if !highlight_code_blocks(blocks, language_registry, cancellation) {
                    return false;
                }
            }
            Block::List { items, .. } => {
                for item in items {
                    if !highlight_code_blocks(item, language_registry, cancellation) {
                        return false;
                    }
                }
            }
            Block::Heading { .. }
            | Block::Paragraph(_)
            | Block::Code { language: None, .. }
            | Block::Table { .. }
            | Block::Image { .. }
            | Block::Math { .. }
            | Block::Rule => {}
        }
    }
    !cancellation.is_cancelled()
}

fn render_code_line(
    line: &str,
    line_range: Range<usize>,
    highlights: Option<&SnippetHighlights>,
    styles: Option<&[HighlightStyle]>,
    highlight_index: &mut usize,
) -> AnyElement {
    let line_highlights: Vec<(Range<usize>, HighlightStyle)> = highlights
        .zip(styles)
        .map(|(highlights, styles)| {
            visible_highlights_for_line(&highlights.spans, line_range, highlight_index)
                .into_iter()
                .filter_map(|(range, capture)| {
                    styles
                        .get(capture as usize)
                        .copied()
                        .map(|style| (range, style))
                })
                .collect()
        })
        .unwrap_or_default();
    StyledText::new(line.to_owned())
        .with_highlights(line_highlights)
        .into_any_element()
}

/// 返回与当前行相交的高亮，并将游标推进到后续行无需再次检查的位置。
///
/// 高亮跨度按文档顺序且互不重叠。
/// 跨行跨度会保留在游标位置，直到其末行处理完毕。
fn visible_highlights_for_line(
    spans: &[HighlightSpan],
    line_range: Range<usize>,
    index: &mut usize,
) -> Vec<(Range<usize>, u32)> {
    while spans
        .get(*index)
        .is_some_and(|span| span.range.end <= line_range.start)
    {
        *index += 1;
    }

    let mut line_highlights = Vec::new();
    let mut current = *index;
    while let Some(span) = spans.get(current) {
        if span.range.start >= line_range.end {
            break;
        }
        let start = span.range.start.max(line_range.start);
        let end = span.range.end.min(line_range.end);
        if start < end {
            line_highlights.push((
                start - line_range.start..end - line_range.start,
                span.capture,
            ));
        }
        if span.range.end > line_range.end {
            break;
        }
        current += 1;
    }
    *index = current;
    line_highlights
}

fn code_lines(text: &str) -> impl Iterator<Item = &str> {
    text.strip_suffix('\n').unwrap_or(text).split('\n')
}

fn heading_size(level: u8, type_scale: typography::Typography) -> gpui::Pixels {
    type_scale.content_size() * heading_scale(level)
}

fn heading_line_height(
    level: u8,
    size: gpui::Pixels,
    type_scale: typography::Typography,
) -> gpui::Pixels {
    // 标题继承用户的正文行高比例，但至少为自身字号保留可读的自然行距。
    (type_scale.content_line() * heading_scale(level)).max(size * 1.2)
}

fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 1.8,
        2 => 1.5,
        3 => 1.3,
        _ => 1.1,
    }
}

impl Item for MarkdownPreviewView {
    type Event = MarkdownPreviewEvent;

    fn tab_content_text(&self, cx: &App) -> SharedString {
        self.source_item
            .item_path(cx)
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Markdown Preview".to_owned())
            .into()
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        match event {
            MarkdownPreviewEvent::SourceMetadataChanged => {
                emit(ItemEvent::PathChanged);
                emit(ItemEvent::UpdateTab);
                emit(ItemEvent::UpdateBreadcrumbs);
            }
        }
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.source_item.is_dirty(cx)
    }

    fn item_path(&self, cx: &App) -> Option<PathBuf> {
        self.source_item.item_path(cx)
    }

    fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        self.source_item.rename_path(from, to, cx);
    }

    fn multi_buffer(&self, _cx: &App) -> Option<Entity<MultiBuffer>> {
        Some(self.multi_buffer.clone())
    }

    fn as_preview_item(
        &self,
        self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn PreviewItemHandle>> {
        Some(Box::new(self_handle.clone()))
    }

    fn can_save(&self, cx: &App) -> bool {
        self.source_item.can_save(cx)
    }

    fn save(
        &mut self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Task<anyhow::Result<()>> {
        self.source_item.save(project, window, cx)
    }

    fn act_as_type(
        &self,
        type_id: TypeId,
        self_handle: &Entity<Self>,
        cx: &App,
    ) -> Option<AnyEntity> {
        if type_id == TypeId::of::<Self>() {
            Some(self_handle.clone().into())
        } else {
            self.source_item.act_as_type(type_id, cx)
        }
    }
}

impl PreviewItem for MarkdownPreviewView {
    fn source_item(&self, _cx: &App) -> Option<Box<dyn ItemHandle>> {
        Some(self.source_item.boxed_clone())
    }
}

#[cfg(test)]
#[path = "test/view_tests.rs"]
mod tests;
