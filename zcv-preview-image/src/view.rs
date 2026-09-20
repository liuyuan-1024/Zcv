//! 栅格图片预览 Item。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, Image, ObjectFit, Render, RenderImage,
    SharedString, Size, Task, Window, div, img, prelude::*,
};
use zcv_theme::color;
use zcv_workspace::{Item, ItemEvent, PreviewViewport, PreviewViewportOptions, SerializedPaneItem};

use crate::provider::image_format_for_path;

enum ImagePreviewState {
    Loading,
    Ready(Arc<RenderImage>),
    Error(String),
}

pub(crate) struct ImagePreviewView {
    path: PathBuf,
    focus: FocusHandle,
    state: ImagePreviewState,
    viewport: PreviewViewport,
    load_task: Option<Task<()>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImagePreviewEvent {
    PathChanged,
}

impl ImagePreviewView {
    pub(crate) fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            path,
            focus: cx.focus_handle(),
            state: ImagePreviewState::Loading,
            viewport: PreviewViewport::new(),
            load_task: None,
        };
        view.start_load(cx);
        view
    }

    fn start_load(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let Some(format) = image_format_for_path(&path) else {
            self.state = ImagePreviewState::Error("不支持的图片格式".to_owned());
            self.load_task = None;
            cx.notify();
            return;
        };
        let renderer = cx.svg_renderer();
        let load = cx.background_spawn(async move {
            let bytes = std::fs::read(&path).map_err(|error| format!("读取图片失败：{error}"))?;
            let image = Image::from_bytes(format, bytes);
            image
                .to_image_data(renderer)
                .map_err(|error| format!("解码图片失败：{error:#}"))
        });
        self.state = ImagePreviewState::Loading;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |view, cx| {
                view.load_task = None;
                view.state = match result {
                    Ok(image) => ImagePreviewState::Ready(image),
                    Err(error) => ImagePreviewState::Error(error),
                };
                view.viewport.invalidate_centering();
                cx.notify();
            });
        }));
        cx.notify();
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
        let (content_size, image, error) = match &self.state {
            ImagePreviewState::Loading => (None, None, None),
            ImagePreviewState::Ready(image) => (
                Some(image.size(0).to_pixels(window.scale_factor())),
                Some(image.clone()),
                None,
            ),
            ImagePreviewState::Error(error) => (None, None, Some(error.clone())),
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

    fn breadcrumbs(
        &self,
        project_root: Option<&Path>,
        _cx: &App,
    ) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        let path = project_root
            .and_then(|root| self.path.strip_prefix(root).ok())
            .unwrap_or(&self.path);
        let segments = path
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned().into())
            .collect();
        Some((segments, None))
    }

    fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        if let Ok(relative) = self.path.strip_prefix(from) {
            self.path = to.join(relative);
            cx.emit(ImagePreviewEvent::PathChanged);
            cx.notify();
        }
    }
}

#[cfg(test)]
#[path = "test/view_tests.rs"]
mod tests;
