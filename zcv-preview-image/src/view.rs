//! 栅格图片预览 Item。
//!
//! 图片内容由 `Project` 拥有的 `ImageDocument` 承担；
//! 本视图只订阅文档重载事件并渲染其内容句柄。

use std::path::{Path, PathBuf};

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, ObjectFit, Render, SharedString,
    Size, Subscription, Window, div, img, prelude::*,
};
use zcv_project::{ImageDocument, ImageDocumentEvent, Project};
use zcv_theme::color;
use zcv_workspace::{
    Item, ItemEvent, PreviewItem, PreviewItemHandle, PreviewViewport, PreviewViewportOptions,
    SerializedPaneItem,
};

pub(crate) struct ImagePreviewView {
    path: PathBuf,
    project: Entity<Project>,
    focus: FocusHandle,
    image: Option<Entity<ImageDocument>>,
    error: Option<String>,
    viewport: PreviewViewport,
    _subscription: Option<Subscription>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImagePreviewEvent {
    PathChanged,
}

impl ImagePreviewView {
    pub(crate) fn new(path: PathBuf, project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let (image, error, subscription) =
            match project.update(cx, |project, cx| project.open_image(&path, cx)) {
                Ok(image) => {
                    let subscription = cx.subscribe(&image, |_, _, _: &ImageDocumentEvent, cx| {
                        cx.notify();
                    });
                    (Some(image), None, Some(subscription))
                }
                Err(error) => (None, Some(format!("打开图片失败：{error:#}")), None),
            };
        Self {
            path,
            project,
            focus: cx.focus_handle(),
            image,
            error,
            viewport: PreviewViewport::new(),
            _subscription: subscription,
        }
    }
}

impl EventEmitter<ImagePreviewEvent> for ImagePreviewView {}

impl Focusable for ImagePreviewView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ImagePreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_text_color = color::current(cx).text_muted;
        let (content_size, image, error) = if let Some(document) = &self.image {
            let document = document.read(cx);
            match (document.image(), document.error()) {
                (Some(image), _) => (
                    Some(image.size(0).to_pixels(window.scale_factor())),
                    Some(image),
                    None,
                ),
                (None, Some(error)) => (None, None, Some(error.to_owned())),
                (None, None) => (None, None, None),
            }
        } else {
            (None, None, self.error.clone())
        };

        self.viewport.render(
            content_size,
            move |display_size| match (image, error, display_size) {
                (Some(image), _, Some(Size { width, height })) => img(image)
                    .w(width)
                    .h(height)
                    .object_fit(ObjectFit::Contain)
                    .flex_none()
                    .into_any_element(),
                (_, Some(error), _) => div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_color(muted_text_color)
                    .child("无法预览图片")
                    .child(error)
                    .into_any_element(),
                _ => div().size_full().into_any_element(),
            },
            PreviewViewportOptions::new(
                &self.focus,
                "ImagePreview",
                "image-preview-scroll-container",
                "image-preview-horizontal-scroll-container",
                color::current(cx).editor_background,
            ),
            window,
            cx,
        )
    }
}

impl Item for ImagePreviewView {
    type Event = ImagePreviewEvent;

    fn tab_content_text(&self, _cx: &App) -> SharedString {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "图片预览".to_owned())
            .into()
    }

    fn tab_icon(&self, _cx: &App) -> Option<SharedString> {
        Some("icons/image.svg".into())
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        match event {
            ImagePreviewEvent::PathChanged => {
                emit(ItemEvent::PathChanged);
                emit(ItemEvent::UpdateTab);
                emit(ItemEvent::UpdateBreadcrumbs);
            }
        }
    }

    fn item_path(&self, _cx: &App) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn serialized_pane_item(&self, _cx: &App) -> Option<SerializedPaneItem> {
        Some(SerializedPaneItem::StandalonePreview(self.path.clone()))
    }

    fn as_preview_item(
        &self,
        self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn PreviewItemHandle>> {
        Some(Box::new(self_handle.clone()))
    }

    fn breadcrumbs(
        &self,
        relative_path: Option<&Path>,
        _cx: &App,
    ) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        let path = relative_path.unwrap_or(&self.path);
        let segments = path
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned().into())
            .collect();
        Some((segments, None))
    }

    fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        if let Ok(relative) = self.path.strip_prefix(from) {
            self.path = to.join(relative);
            match self
                .project
                .update(cx, |project, cx| project.open_image(&self.path, cx))
            {
                Ok(image) => {
                    self._subscription =
                        Some(cx.subscribe(&image, |_, _, _: &ImageDocumentEvent, cx| cx.notify()));
                    self.image = Some(image);
                    self.error = None;
                }
                Err(error) => {
                    self.image = None;
                    self._subscription = None;
                    self.error = Some(format!("打开图片失败：{error:#}"));
                }
            }
            cx.emit(ImagePreviewEvent::PathChanged);
            cx.notify();
        }
    }
}

impl PreviewItem for ImagePreviewView {}

#[cfg(test)]
#[path = "test/view_tests.rs"]
mod tests;
