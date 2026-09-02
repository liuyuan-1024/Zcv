//! Markdown 预览 Item：观察源码 MultiBuffer，解析后渲染为原生块元素。

use std::any::TypeId;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AnyEntity, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    Render, ScrollHandle, SharedString, Subscription, Window, div, prelude::*, px,
};
use zcv_multi_buffer::MultiBuffer;
use zcv_project::Project;
use zcv_theme::{color, space, typography};
use zcv_workspace::{
    Item, ItemEvent, ItemHandle, PreviewDocument, PreviewItem, PreviewItemHandle,
    ToolbarItemLocation,
};

use crate::document::{Block, parse};

pub(crate) struct MarkdownPreviewView {
    source_item: Box<dyn ItemHandle>,
    multi_buffer: Entity<MultiBuffer>,
    focus: FocusHandle,
    scroll_handle: ScrollHandle,
    blocks: Vec<Block>,
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
            view.refresh(cx);
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
            _document_subscription: document_subscription,
            _item_subscription: item_subscription,
        };
        view.refresh(cx);
        view
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let text = String::from_utf8(self.multi_buffer.read(cx).snapshot(cx).text_bytes())
            .expect("编辑器文档应为 UTF-8");
        self.blocks = parse(&text);
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
        let content = self
            .blocks
            .iter()
            .map(|block| render_block(block, cx))
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

fn render_block(block: &Block, cx: &App) -> AnyElement {
    match block {
        Block::Heading { level, text } => div()
            .text_size(heading_size(*level))
            .font_weight(FontWeight::BOLD)
            .child(text.clone())
            .into_any_element(),
        Block::Paragraph(text) => div()
            .whitespace_normal()
            .child(text.clone())
            .into_any_element(),
        Block::Code(text) => div()
            .rounded_md()
            .bg(color::current(cx).panel_background)
            .p_3()
            .font(typography::content_font())
            .text_size(typography::content_size())
            .flex()
            .flex_col()
            .children(code_lines(text).map(|line| div().child(line.to_owned())))
            .into_any_element(),
        Block::Quote(text) => div()
            .border_l_2()
            .border_color(color::current(cx).border_variant)
            .pl_3()
            .text_color(color::current(cx).text_muted)
            .child(text.clone())
            .into_any_element(),
        Block::List { start, items } => div()
            .flex()
            .flex_col()
            .gap_1()
            .children(items.iter().enumerate().map(|(index, item)| {
                let marker = start.map_or_else(
                    || "•".to_owned(),
                    |start| format!("{}.", start + index as u64),
                );
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .w_5()
                            .text_color(color::current(cx).text_muted)
                            .child(marker),
                    )
                    .child(div().flex_1().child(item.clone()))
            }))
            .into_any_element(),
        Block::Rule => div()
            .h(px(1.))
            .w_full()
            .bg(color::current(cx).border_variant)
            .into_any_element(),
    }
}

fn code_lines(text: &str) -> impl Iterator<Item = &str> {
    text.strip_suffix('\n').unwrap_or(text).split('\n')
}

fn heading_size(level: u8) -> gpui::Pixels {
    match level {
        1 => typography::content_size() * 1.8,
        2 => typography::content_size() * 1.5,
        3 => typography::content_size() * 1.3,
        _ => typography::content_size() * 1.1,
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

    use super::{Block, MarkdownPreviewView, code_lines};

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
        cx.read_entity(&view, |view, _| {
            assert_eq!(
                view.blocks,
                vec![Block::Heading {
                    level: 1,
                    text: "初始标题".into(),
                }]
            );
        });

        editor.update(cx, |editor, cx| editor.set_text("更新后的正文", cx));
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.blocks, vec![Block::Paragraph("更新后的正文".into())]);
        });
    }
}
