//! Markdown 预览 Item：观察源码 MultiBuffer，解析后渲染为原生块元素。

use std::any::TypeId;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    AnyElement, AnyEntity, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontStyle,
    FontWeight, HighlightStyle, InteractiveText, ObjectFit, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, StrikethroughStyle, StyledImage, StyledText, Subscription, Task,
    UnderlineStyle, Window, div, img, prelude::*, px,
};
use pulldown_cmark::Alignment;
use zcv_language::highlight_snippet;
use zcv_multi_buffer::MultiBuffer;
use zcv_project::Project;
use zcv_theme::{color, space, typography};
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
    blocks: Vec<Block>,
    code_highlight_generation: u64,
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
        let mut view = Self {
            source_item,
            multi_buffer,
            focus: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            blocks: Vec::new(),
            code_highlight_generation: 0,
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
        let text = String::from_utf8(self.multi_buffer.read(cx).snapshot(cx).text_bytes())
            .expect("编辑器文档应为 UTF-8");
        self.blocks = parse(&text);
        self.code_highlight_generation = self.code_highlight_generation.wrapping_add(1);
        let generation = self.code_highlight_generation;
        let mut blocks = self.blocks.clone();
        let highlights = cx.background_spawn(async move {
            highlight_code_blocks(&mut blocks);
            blocks
        });
        self.code_highlight_task = Some(cx.spawn(async move |this, cx| {
            let blocks = highlights.await;
            let _ = this.update(cx, |view, cx| {
                if view.code_highlight_generation != generation {
                    return;
                }
                view.blocks = blocks;
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
        let source_directory = source_path.as_deref().and_then(Path::parent);
        let mut next_key = 0;
        let content = self
            .blocks
            .iter()
            .map(|block| render_block(block, &mut next_key, source_directory, 0, cx))
            .collect::<Vec<_>>();
        div()
            .id("markdown-preview")
            .track_focus(&self.focus)
            .key_context("MarkdownPreview")
            .tab_index(0)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .bg(color::current(cx).editor_background)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(space::S8)
                    .px_8()
                    .py_6()
                    .text_color(color::current(cx).text)
                    .text_size(typography::content_size())
                    .line_height(typography::content_line())
                    .children(content),
            )
    }
}

fn render_block(
    block: &Block,
    next_key: &mut usize,
    source_directory: Option<&Path>,
    list_depth: usize,
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
                .map(|highlights| zcv_theme::syntax::style_table(&highlights.capture_names));
            let mut line_start = 0;
            for line in code_lines(text) {
                let line_end = line_start + line.len();
                code = code.child(div().child(render_code_line(
                    line,
                    line_start..line_end,
                    highlights.as_ref(),
                    styles.as_deref(),
                )));
                line_start = line_end + 1;
            }
            code.into_any_element()
        }
        Block::Quote(blocks) => {
            let children = blocks
                .iter()
                .map(|block| render_block(block, next_key, source_directory, list_depth, cx))
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
                            render_block(block, next_key, source_directory, list_depth + 1, cx)
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
                .id(("markdown-table", key))
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

fn highlight_code_blocks(blocks: &mut [Block]) {
    for block in blocks {
        match block {
            Block::Code {
                language: Some(language),
                text,
                highlights,
            } => *highlights = highlight_snippet(language, text),
            Block::Quote(blocks) => highlight_code_blocks(blocks),
            Block::List { items, .. } => {
                for item in items {
                    highlight_code_blocks(item);
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
}

fn render_code_line(
    line: &str,
    line_range: Range<usize>,
    highlights: Option<&zcv_language::SnippetHighlights>,
    styles: Option<&[HighlightStyle]>,
) -> AnyElement {
    let line_highlights: Vec<(Range<usize>, HighlightStyle)> = highlights
        .into_iter()
        .flat_map(|highlights| highlights.spans.iter())
        .filter_map(|span| {
            let start = span.range.start.max(line_range.start);
            let end = span.range.end.min(line_range.end);
            if start >= end {
                return None;
            }
            let style = *styles?.get(span.capture as usize)?;
            Some((start - line_range.start..end - line_range.start, style))
        })
        .collect();
    StyledText::new(line.to_owned())
        .with_highlights(line_highlights)
        .into_any_element()
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
        heading_size, highlight_code_blocks, list_marker_char_count,
    };

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
        highlight_code_blocks(&mut blocks);
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
                view.blocks,
                vec![Block::Heading {
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
                view.blocks,
                vec![Block::Paragraph(vec![plain("更新后的正文")])]
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
            assert_eq!(view.blocks, vec![Block::Paragraph(vec![plain("初始正文")])]);
        });

        cx.executor().advance_clock(MARKDOWN_REPARSE_DEBOUNCE);
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.blocks, vec![Block::Paragraph(vec![plain("最终内容")])]);
        });
    }
}
