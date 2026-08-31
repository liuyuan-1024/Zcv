//! SVG 预览视图：把源码 Item 的 MultiBuffer 内容栅格化为图像展示。
//!
//! 标签元数据（标题、路径、脏状态等）全部转发给源码 Item；
//! [`Item::source_item`] 让 Pane 能在预览与源码之间切换而不依赖具体视图类型。

use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyEntity, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Image, ImageFormat,
    IntoElement, ObjectFit, Render, RenderImage, SharedString, Styled, StyledImage, Subscription,
    Task, Window, div, img, prelude::*,
};
use zcv_multi_buffer::MultiBuffer;
use zcv_project::Project;
use zcv_theme::color;
use zcv_workspace::{
    Item, ItemEvent, ItemHandle, PreviewDocument, PreviewItem, PreviewItemHandle,
    ToolbarItemLocation,
};

use crate::renderer::rasterize_svg;

enum SvgPreviewState {
    Loading,
    Ready(Arc<RenderImage>),
    Error(String),
}

pub(crate) struct SvgPreviewView {
    /// 源码 Item（通常是编辑器），渲染数据源与标签元数据都转发给它。
    source_item: Box<dyn ItemHandle>,
    multi_buffer: Entity<MultiBuffer>,
    resources_dir: Option<PathBuf>,
    focus: FocusHandle,
    state: SvgPreviewState,
    render_generation: u64,
    render_task: Option<Task<()>>,
    _document_subscription: Subscription,
    _item_subscription: Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SvgPreviewEvent {
    SourcePathChanged,
}

impl SvgPreviewView {
    pub(crate) fn new(document: PreviewDocument, cx: &mut Context<Self>) -> Self {
        let source_item = document.source_item;
        let multi_buffer = document.multi_buffer;
        let resources_dir = document.path.parent().map(PathBuf::from);
        let document_subscription = cx.observe(&multi_buffer, |view, _, cx| {
            view.start_render(cx);
        });
        // 源码路径变化（UpdateBreadcrumbs）时刷新渲染资源目录并重新渲染。
        let this = cx.entity().downgrade();
        let item = source_item.boxed_clone();
        let item_subscription = source_item.subscribe_to_item_events(
            cx,
            Box::new(move |event, cx| {
                if matches!(event, ItemEvent::UpdateBreadcrumbs)
                    && let Some(path) = item.item_path(cx)
                {
                    this.update(cx, |view, cx| {
                        view.resources_dir = path.parent().map(PathBuf::from);
                        view.start_render(cx);
                        cx.emit(SvgPreviewEvent::SourcePathChanged);
                    })
                    .ok();
                }
            }),
        );
        let mut view = Self {
            source_item,
            multi_buffer,
            resources_dir,
            focus: cx.focus_handle(),
            state: SvgPreviewState::Loading,
            render_generation: 0,
            render_task: None,
            _document_subscription: document_subscription,
            _item_subscription: item_subscription,
        };
        view.start_render(cx);
        view
    }

    fn start_render(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.multi_buffer.read(cx).snapshot(cx);
        let bytes = snapshot.text_bytes();
        let version = snapshot.version();
        let resources_dir = self.resources_dir.clone();
        self.render_generation = self.render_generation.wrapping_add(1);
        let generation = self.render_generation;
        self.state = SvgPreviewState::Loading;

        let rasterize_task =
            cx.background_spawn(async move { rasterize_svg(&bytes, resources_dir) });
        self.render_task = Some(cx.spawn(async move |this, cx| {
            let rasterized = rasterize_task.await;
            let _ = this.update(cx, |view, cx| {
                if view.render_generation != generation
                    || view.multi_buffer.read(cx).snapshot(cx).version() != version
                {
                    return;
                }
                view.render_task = None;
                view.state = match rasterized {
                    Ok(rasterized) => Image::from_bytes(ImageFormat::Png, rasterized.png)
                        .to_image_data(cx.svg_renderer())
                        .map(SvgPreviewState::Ready)
                        .unwrap_or_else(|error| SvgPreviewState::Error(error.to_string())),
                    Err(error) => SvgPreviewState::Error(error),
                };
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl EventEmitter<SvgPreviewEvent> for SvgPreviewView {}

impl Focusable for SvgPreviewView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SvgPreviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let preview = match &self.state {
            SvgPreviewState::Loading => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(color::current(cx).text_muted)
                .child("正在渲染 SVG…")
                .into_any_element(),
            SvgPreviewState::Ready(image) => img(image.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            SvgPreviewState::Error(error) => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(color::current(cx).text_muted)
                .child("无法渲染 SVG")
                .child(error.clone())
                .into_any_element(),
        };
        div()
            .track_focus(&self.focus)
            .key_context("ImageViewer")
            .tab_index(0)
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .p_4()
            .bg(color::current(cx).editor_background)
            .child(preview)
    }
}

impl Item for SvgPreviewView {
    type Event = SvgPreviewEvent;

    fn tab_content_text(&self, cx: &App) -> SharedString {
        self.source_item
            .item_path(cx)
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "SVG Preview".to_owned())
            .into()
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        match event {
            SvgPreviewEvent::SourcePathChanged => emit(ItemEvent::UpdateBreadcrumbs),
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

    fn breadcrumbs(&self, cx: &App) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        let (mut segments, font) = self.source_item.breadcrumbs(cx)?;
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
    ) -> Task<anyhow::Result<()>> {
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

impl PreviewItem for SvgPreviewView {
    fn source_item(&self, _cx: &App) -> Option<Box<dyn ItemHandle>> {
        Some(self.source_item.boxed_clone())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui::TestAppContext;
    use zcv_editor::Editor;

    use super::*;

    #[gpui::test]
    fn preview_starts_loading_and_installs_background_result(cx: &mut TestAppContext) {
        let editor = cx.new(Editor::single_line);
        editor.update(cx, |editor, cx| {
            editor.set_text(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"/>"#,
                cx,
            );
            editor.set_file_path(PathBuf::from("icon.svg"), cx);
        });
        let multi_buffer = cx.read_entity(&editor, |editor, _| editor.multi_buffer());
        let view = cx.new(|cx| {
            SvgPreviewView::new(
                PreviewDocument {
                    path: PathBuf::from("icon.svg"),
                    source_item: Box::new(editor),
                    multi_buffer,
                },
                cx,
            )
        });

        cx.read_entity(&view, |view, _| {
            assert!(matches!(view.state, SvgPreviewState::Loading));
            assert!(view.render_task.is_some());
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(matches!(view.state, SvgPreviewState::Ready(_)));
            assert!(view.render_task.is_none());
        });
    }
}
