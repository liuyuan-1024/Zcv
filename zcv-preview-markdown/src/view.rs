//! Markdown 预览 Item：观察源码 MultiBuffer，解析后渲染为原生块元素。

use std::any::TypeId;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, AnyEntity, App, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    FontStyle, FontWeight, HighlightStyle, InteractiveText, ObjectFit, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, StrikethroughStyle, StyledImage, StyledText,
    Subscription, Task, UnderlineStyle, Window, div, img, prelude::*, px,
};
use pulldown_cmark::Alignment;
use zcv_language::{
    HighlightSpan, SnippetHighlightCancellation, SnippetHighlights,
    highlight_snippet_with_cancellation,
};
use zcv_multi_buffer::MultiBuffer;
use zcv_project::Project;
use zcv_theme::{color, space, syntax, typography};
use zcv_ui::Scrollbar;
use zcv_workspace::{
    Item, ItemEvent, ItemHandle, PreviewDocument, PreviewItem, PreviewItemHandle,
    ToolbarItemLocation,
};

use crate::document::{Block, Inline, parse};

const MARKDOWN_REPARSE_DEBOUNCE: Duration = Duration::from_millis(200);

pub(crate) struct MarkdownPreviewView {
    source_item: Box<dyn ItemHandle>,
    multi_buffer: Entity<MultiBuffer>,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkdownPreviewEvent {
    SourceMetadataChanged,
}

impl MarkdownPreviewView {
    pub(crate) fn new(document: PreviewDocument, cx: &mut Context<Self>) -> Self {
        let source_item = document.source_item;
        let multi_buffer = document.multi_buffer;
        let document_subscription = cx.observe(&multi_buffer, |view, _, cx| {
            view.schedule_refresh(cx);
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
        let mut view = Self {
            source_item,
            multi_buffer,
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
        let text = String::from_utf8(self.multi_buffer.read(cx).snapshot(cx).text_bytes())
            .expect("编辑器文档应为 UTF-8");
        self.blocks = Arc::new(parse(&text));
        self.code_highlight_generation = self.code_highlight_generation.wrapping_add(1);
        let generation = self.code_highlight_generation;
        let cancellation = SnippetHighlightCancellation::default();
        self.code_highlight_cancellation = Some(cancellation.clone());
        let mut blocks = (*self.blocks).clone();
        let highlights = cx.background_spawn(async move {
            highlight_code_blocks(&mut blocks, &cancellation).then_some(blocks)
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
}

impl EventEmitter<MarkdownPreviewEvent> for MarkdownPreviewView {}

impl Focusable for MarkdownPreviewView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MarkdownPreviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let source_path = self.source_item.item_path(cx);
        let source_directory = source_path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let content = self
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| {
                let mut next_key = 0;
                div()
                    .pb(space::S8)
                    .child(render_block(
                        block,
                        &mut next_key,
                        source_directory.as_deref(),
                        0,
                        index,
                        cx,
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
                    .px_8()
                    .py_6()
                    .text_color(color::current(cx).text)
                    .text_size(typography::content_size())
                    .line_height(typography::content_line())
                    .child(div().w_full().flex().flex_col().children(content)),
            )
            .child(div().absolute().inset_0().child(self.scrollbar.clone()))
    }
}

fn render_block(
    block: &Block,
    next_key: &mut usize,
    source_directory: Option<&Path>,
    list_depth: usize,
    namespace: usize,
    cx: &App,
) -> AnyElement {
    let key = *next_key;
    *next_key += 1;
    match block {
        Block::Heading { level, content } => {
            let size = heading_size(*level);
            div()
                .text_size(size)
                .line_height(heading_line_height(*level, size))
                .font_weight(FontWeight::BOLD)
                .child(render_inline(content, key, cx))
                .into_any_element()
        }
        Block::Paragraph(content) => div()
            .whitespace_normal()
            .child(render_inline(content, key, cx))
            .into_any_element(),
        Block::Code {
            language,
            text,
            highlights,
        } => {
            let mut code = div()
                .rounded_md()
                .bg(color::current(cx).panel_background)
                .p_3()
                .font(typography::content_font())
                .text_size(typography::content_size())
                .flex()
                .flex_col();
            if let Some(language) = language {
                code = code.child(
                    div()
                        .mb_2()
                        .text_size(typography::content_size() * 0.85)
                        .text_color(color::current(cx).text_muted)
                        .child(language.clone()),
                );
            }
            let styles = highlights
                .as_ref()
                .map(|highlights| syntax::style_table(&highlights.capture_names));
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
                .map(|block| {
                    render_block(block, next_key, source_directory, list_depth, namespace, cx)
                })
                .collect::<Vec<_>>();
            div()
                .border_l_2()
                .border_color(color::current(cx).border_variant)
                .pl_3()
                .flex()
                .flex_col()
                .gap_2()
                .text_color(color::current(cx).text_muted)
                .children(children)
                .into_any_element()
        }
        Block::List { start, items } => {
            let marker_width = list_marker_width(*start, items.len());
            let children = items
                .iter()
                .enumerate()
                .map(|(item_index, item)| {
                    let marker = start.map_or_else(
                        || "•".to_owned(),
                        |start| format!("{}.", start + item_index as u64),
                    );
                    let item_children = item
                        .iter()
                        .map(|block| {
                            render_block(
                                block,
                                next_key,
                                source_directory,
                                list_depth + 1,
                                namespace,
                                cx,
                            )
                        })
                        .collect::<Vec<_>>();
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            div()
                                .w(marker_width)
                                .flex_none()
                                .text_left()
                                .text_color(color::current(cx).text_muted)
                                .child(marker),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .children(item_children),
                        )
                })
                .collect::<Vec<_>>();
            div()
                .when(list_depth > 0, |list| list.pl_4())
                .flex()
                .flex_col()
                .gap_1()
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
                table = table.child(render_table_row(header, alignments, true, next_key, cx));
            }
            for row in rows {
                table = table.child(render_table_row(row, alignments, false, next_key, cx));
            }
            table.into_any_element()
        }
        Block::Image { source, alt } => render_image(source, alt, source_directory, cx),
        Block::Rule => div()
            .h(px(1.))
            .w_full()
            .bg(color::current(cx).border_variant)
            .into_any_element(),
    }
}

fn list_marker_width(start: Option<u64>, item_count: usize) -> gpui::Pixels {
    let marker_char_count = list_marker_char_count(start, item_count);
    // 标记列按字符数预留，正文与编号之间只保留布局间距。
    typography::content_size() * (marker_char_count as f32 * 0.6)
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
    .w_full()
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
    div().w_full().child(image).into_any_element()
}

fn render_table_row(
    cells: &[Vec<Inline>],
    alignments: &[Alignment],
    is_header: bool,
    next_key: &mut usize,
    cx: &App,
) -> AnyElement {
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
                .min_w_32()
                .px_3()
                .py_2()
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
            cell.child(render_inline(cell_content, key, cx))
        }))
        .into_any_element()
}

fn render_inline(content: &[Inline], key: usize, cx: &App) -> AnyElement {
    let mut text = String::new();
    let mut highlights = Vec::new();
    let mut links = Vec::new();
    let mut link_ranges = Vec::new();

    for inline in content {
        let start = text.len();
        text.push_str(&inline.text);
        let end = text.len();
        if start == end {
            continue;
        }
        let style = &inline.style;
        if style.emphasis
            || style.strong
            || style.strikethrough
            || style.code
            || style.link.is_some()
        {
            highlights.push((
                start..end,
                HighlightStyle {
                    font_style: style.emphasis.then_some(FontStyle::Italic),
                    font_weight: style.strong.then_some(FontWeight::BOLD),
                    strikethrough: style.strikethrough.then_some(StrikethroughStyle {
                        thickness: px(2.),
                        color: Some(color::current(cx).text.into()),
                    }),
                    background_color: style
                        .code
                        .then_some(color::current(cx).surface_background.into()),
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
    if links.is_empty() {
        text.into_any_element()
    } else {
        InteractiveText::new(("markdown-link", key), text)
            .on_click(link_ranges, move |index, _window, cx| {
                cx.open_url(&links[index])
            })
            .into_any_element()
    }
}

fn highlight_code_blocks(
    blocks: &mut [Block],
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
            } => *highlights = highlight_snippet_with_cancellation(language, text, cancellation),
            Block::Quote(blocks) => {
                if !highlight_code_blocks(blocks, cancellation) {
                    return false;
                }
            }
            Block::List { items, .. } => {
                for item in items {
                    if !highlight_code_blocks(item, cancellation) {
                        return false;
                    }
                }
            }
            Block::Heading { .. }
            | Block::Paragraph(_)
            | Block::Code { language: None, .. }
            | Block::Table { .. }
            | Block::Image { .. }
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

fn heading_size(level: u8) -> gpui::Pixels {
    typography::content_size() * heading_scale(level)
}

fn heading_line_height(level: u8, size: gpui::Pixels) -> gpui::Pixels {
    // 标题继承用户的正文行高比例，但至少为自身字号保留可读的自然行距。
    (typography::content_line() * heading_scale(level)).max(size * 1.2)
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

    fn breadcrumb_location(&self, _cx: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::PrimaryLeft
    }

    fn breadcrumbs(
        &self,
        project_root: Option<&Path>,
        cx: &App,
    ) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        let (mut segments, font) = self.source_item.breadcrumbs(project_root, cx)?;
        segments.push("Preview".into());
        Some((segments, font))
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
mod tests {
    use std::path::PathBuf;

    use gpui::{AppContext, TestAppContext};
    use zcv_editor::Editor;
    use zcv_workspace::PreviewDocument;

    use crate::document::{Inline, InlineStyle, parse};

    use super::{
        Block, MARKDOWN_REPARSE_DEBOUNCE, MarkdownPreviewView, code_lines, heading_line_height,
        heading_size, highlight_code_blocks, list_marker_char_count, visible_highlights_for_line,
    };
    use zcv_language::{HighlightSpan, SnippetHighlightCancellation};

    fn plain(text: &str) -> Inline {
        Inline {
            text: text.into(),
            style: InlineStyle::default(),
        }
    }

    #[test]
    fn code_lines_omits_parser_terminator_without_dropping_blank_lines() {
        assert_eq!(
            code_lines("let x = 1;\n").collect::<Vec<_>>(),
            ["let x = 1;"]
        );
        assert_eq!(
            code_lines("let x = 1;\n\n").collect::<Vec<_>>(),
            ["let x = 1;", ""]
        );
    }

    #[test]
    fn code_line_highlights_scan_each_span_once_except_for_its_covered_lines() {
        let spans = [
            HighlightSpan {
                range: 2..5,
                capture: 1,
            },
            HighlightSpan {
                range: 8..14,
                capture: 2,
            },
            HighlightSpan {
                range: 15..17,
                capture: 3,
            },
        ];
        let mut index = 0;

        assert_eq!(
            visible_highlights_for_line(&spans, 0..4, &mut index),
            vec![(2..4, 1)]
        );
        assert_eq!(
            visible_highlights_for_line(&spans, 5..9, &mut index),
            vec![(3..4, 2)]
        );
        assert_eq!(
            visible_highlights_for_line(&spans, 10..14, &mut index),
            vec![(0..4, 2)]
        );
        assert_eq!(
            visible_highlights_for_line(&spans, 15..18, &mut index),
            vec![(0..2, 3)]
        );
        assert_eq!(index, spans.len());
    }

    #[test]
    fn ordered_list_marker_width_accounts_for_multi_digit_numbers() {
        assert_eq!(list_marker_char_count(None, 3), 1);
        assert_eq!(list_marker_char_count(Some(1), 9), 2);
        assert_eq!(list_marker_char_count(Some(10), 11), 3);
        assert_eq!(list_marker_char_count(Some(98), 3), 4);
    }

    #[test]
    fn headings_preserve_a_minimum_line_height_at_their_own_font_size() {
        for level in 1..=6 {
            let size = heading_size(level);
            assert!(heading_line_height(level, size) >= size * 1.2);
        }
    }

    #[test]
    fn applies_language_highlights_to_fenced_code_blocks() {
        let mut blocks = parse("```rust\nfn main() {}\n```");
        assert!(highlight_code_blocks(
            &mut blocks,
            &SnippetHighlightCancellation::default()
        ));
        assert!(matches!(
            blocks.as_slice(),
            [Block::Code {
                highlights: Some(highlights),
                ..
            }] if !highlights.spans.is_empty()
        ));
    }

    #[gpui::test]
    fn preview_rebuilds_when_source_document_changes(cx: &mut TestAppContext) {
        let editor = cx.new(Editor::single_line);
        editor.update(cx, |editor, cx| {
            editor.set_text("# 初始标题", cx);
            editor.set_file_path(PathBuf::from("README.md"), cx);
        });
        let multi_buffer = cx.read_entity(&editor, |editor, _| editor.multi_buffer());
        let view = cx.new(|cx| {
            MarkdownPreviewView::new(
                PreviewDocument {
                    path: PathBuf::from("README.md"),
                    source_item: Box::new(editor.clone()),
                    multi_buffer,
                },
                cx,
            )
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(
                view.blocks.as_ref(),
                &vec![Block::Heading {
                    level: 1,
                    content: vec![plain("初始标题")],
                }]
            );
        });

        editor.update(cx, |editor, cx| editor.set_text("更新后的正文", cx));
        cx.executor().advance_clock(MARKDOWN_REPARSE_DEBOUNCE);
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(
                view.blocks.as_ref(),
                &vec![Block::Paragraph(vec![plain("更新后的正文")])]
            );
        });
    }

    #[gpui::test]
    fn preview_coalesces_rapid_document_changes(cx: &mut TestAppContext) {
        let editor = cx.new(Editor::single_line);
        editor.update(cx, |editor, cx| {
            editor.set_text("初始正文", cx);
            editor.set_file_path(PathBuf::from("README.md"), cx);
        });
        let multi_buffer = cx.read_entity(&editor, |editor, _| editor.multi_buffer());
        let view = cx.new(|cx| {
            MarkdownPreviewView::new(
                PreviewDocument {
                    path: PathBuf::from("README.md"),
                    source_item: Box::new(editor.clone()),
                    multi_buffer,
                },
                cx,
            )
        });
        cx.run_until_parked();

        editor.update(cx, |editor, cx| editor.set_text("第一次修改", cx));
        editor.update(cx, |editor, cx| editor.set_text("最终内容", cx));
        cx.read_entity(&view, |view, _| {
            assert_eq!(
                view.blocks.as_ref(),
                &vec![Block::Paragraph(vec![plain("初始正文")])]
            );
        });

        cx.executor().advance_clock(MARKDOWN_REPARSE_DEBOUNCE);
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(
                view.blocks.as_ref(),
                &vec![Block::Paragraph(vec![plain("最终内容")])]
            );
        });
    }
}
