//! SVG 预览视图：把源码 Item 的 MultiBuffer 内容栅格化为图像展示。
//!
//! 标签元数据（标题、路径、脏状态等）全部转发给源码 Item；
//! [`Item::source_item`] 让 Pane 能在预览与源码之间切换而不依赖具体视图类型。

use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyEntity, AnyView, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Image,
    ImageFormat, IntoElement, ObjectFit, Render, RenderImage, SharedString, Styled, StyledImage,
    Subscription, Task, Window, div, img, prelude::*, size,
};
use zcv_multi_buffer::MultiBuffer;
use zcv_project::Project;
use zcv_theme::{color, space};
use zcv_ui::Button;
use zcv_workspace::{
    Breadcrumbs, Item, ItemEvent, ItemHandle, PreviewDocument, PreviewItem, PreviewItemHandle,
    PreviewToggleCallback, PreviewViewport, PreviewViewportOptions,
};

use crate::renderer::{SVG_PREVIEW_MIN_DISPLAY_EDGE, rasterize_svg};

enum SvgPreviewState {
    Loading,
    Ready(RenderedSvg),
    Error(String),
}

struct RenderedSvg {
    image: Arc<RenderImage>,
    raster_scale: f32,
}

pub(crate) struct SvgPreviewView {
    /// 源码 Item（通常是编辑器），渲染数据源与标签元数据都转发给它。
    source_item: Box<dyn ItemHandle>,
    multi_buffer: Entity<MultiBuffer>,
    resources_dir: Option<PathBuf>,
    focus: FocusHandle,
    state: SvgPreviewState,
    requested_scale: Option<f32>,
    viewport: PreviewViewport,
    render_generation: u64,
    render_task: Option<Task<()>>,
    _document_subscription: Subscription,
    _item_subscription: Subscription,
    breadcrumbs: Entity<Breadcrumbs>,
    toolbar: Entity<SvgPreviewToolbar>,
}

struct SvgPreviewToolbar {
    breadcrumbs: Entity<Breadcrumbs>,
    toggle_preview: PreviewToggleCallback,
}

impl Render for SvgPreviewToolbar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .items_center()
            .gap(space::S6)
            .child(div().flex_1().min_w_0().child(self.breadcrumbs.clone()))
            .child(
                Button::icon("svg-preview-source", "icons/eye_off.svg")
                    .label("返回源码")
                    .on_click({
                        let toggle_preview = self.toggle_preview.clone();
                        move |_, window, cx| toggle_preview(window, cx)
                    }),
            )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SvgPreviewEvent {
    SourcePathChanged,
}

impl SvgPreviewView {
    pub(crate) fn new(document: PreviewDocument, cx: &mut Context<Self>) -> Self {
        let PreviewDocument::Source {
            path,
            source_item,
            multi_buffer,
            toggle_preview,
            ..
        } = document
        else {
            panic!("SVG 预览必须从源码 Item 创建")
        };
        let breadcrumbs = cx.new(|_| Breadcrumbs::without_project());
        breadcrumbs.update(cx, |view, cx| view.set_item(Some(source_item.as_ref()), cx));
        let toolbar = cx.new(|_| SvgPreviewToolbar {
            breadcrumbs: breadcrumbs.clone(),
            toggle_preview,
        });
        let resources_dir = path.parent().map(PathBuf::from);
        let document_subscription = cx.observe(&multi_buffer, |view, _, cx| {
            view.start_render(1., cx);
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
                        view.breadcrumbs.update(cx, |_, cx| cx.notify());
                        view.start_render(1., cx);
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
            requested_scale: None,
            viewport: PreviewViewport::new(),
            render_generation: 0,
            render_task: None,
            _document_subscription: document_subscription,
            _item_subscription: item_subscription,
            breadcrumbs,
            toolbar,
        };
        view.start_render(1., cx);
        view
    }

    fn start_render(&mut self, content_scale: f32, cx: &mut Context<Self>) {
        let snapshot = self.multi_buffer.read(cx).snapshot(cx);
        // SVG 光栅化需要整份文档文本；这是只读边界，不进入编辑/显示热路径。
        let bytes = snapshot.text_bytes();
        let version = snapshot.version();
        let resources_dir = self.resources_dir.clone();
        let raster_scale = svg_raster_scale(content_scale);
        self.render_generation = self.render_generation.wrapping_add(1);
        let generation = self.render_generation;
        self.requested_scale = Some(content_scale);
        if !matches!(&self.state, SvgPreviewState::Ready(_)) {
            self.state = SvgPreviewState::Loading;
        }

        let renderer = cx.svg_renderer();
        let render_task = cx.background_spawn(async move {
            let rasterized = rasterize_svg(&bytes, resources_dir, raster_scale)?;
            let image = Image::from_bytes(ImageFormat::Png, rasterized.png)
                .to_image_data(renderer)
                .map_err(|error| error.to_string())?;
            Ok(RenderedSvg {
                image,
                raster_scale: rasterized.scale,
            })
        });
        self.render_task = Some(cx.spawn(async move |this, cx| {
            let rendered = render_task.await;
            let _ = this.update(cx, |view, cx| {
                if view.render_generation != generation
                    || view.multi_buffer.read(cx).snapshot(cx).version() != version
                {
                    return;
                }
                view.render_task = None;
                view.state = match rendered {
                    Ok(rendered) => {
                        view.viewport.invalidate_centering();
                        SvgPreviewState::Ready(rendered)
                    }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content_scale = PreviewViewport::content_scale(window, cx);
        if self.requested_scale != Some(content_scale) {
            self.start_render(content_scale, cx);
        }
        let (content_size, image, error) = match &self.state {
            SvgPreviewState::Loading => (None, None, None),
            SvgPreviewState::Ready(rendered) => {
                let image_size = rendered.image.size(0).to_pixels(window.scale_factor());
                let display_scale = svg_display_scale(image_size, rendered.raster_scale);
                (
                    Some(size(
                        image_size.width * display_scale,
                        image_size.height * display_scale,
                    )),
                    Some(rendered.image.clone()),
                    None,
                )
            }
            SvgPreviewState::Error(error) => (None, None, Some(error.clone())),
        };
        let muted_text_color = color::current(cx).text_muted;
        self.viewport.render(
            content_size,
            move |display_size| match (image, error, display_size) {
                (Some(image), _, Some(size)) => img(image)
                    .w(size.width)
                    .h(size.height)
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
                    .child("无法渲染 SVG")
                    .child(error)
                    .into_any_element(),
                _ => div().size_full().into_any_element(),
            },
            PreviewViewportOptions::new(
                &self.focus,
                "ImageViewer",
                "svg-preview-scroll-container",
                "svg-preview-horizontal-scroll-container",
                color::current(cx).editor_background,
            ),
            window,
            cx,
        )
    }
}

fn svg_raster_scale(content_scale: f32) -> f32 {
    content_scale.max(1.)
}

fn svg_display_scale(image_size: gpui::Size<gpui::Pixels>, raster_scale: f32) -> f32 {
    let intrinsic_width = f32::from(image_size.width) / raster_scale;
    let intrinsic_height = f32::from(image_size.height) / raster_scale;
    let intrinsic_longest_edge = intrinsic_width.max(intrinsic_height).max(1.);
    let minimum_size_scale = (SVG_PREVIEW_MIN_DISPLAY_EDGE / intrinsic_longest_edge).max(1.);
    minimum_size_scale / raster_scale
}

impl Item for SvgPreviewView {
    type Event = SvgPreviewEvent;

    fn toolbar_view(&self, _self_handle: &Entity<Self>, _cx: &App) -> Option<AnyView> {
        Some(self.toolbar.clone().into())
    }

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
    use std::rc::Rc;

    use std::path::PathBuf;

    use gpui::{AppContext as _, TestAppContext, px, size};
    use zcv_editor::Editor;

    use super::*;

    #[test]
    fn small_svg_gets_a_readable_initial_display_size() {
        let expected_scale = (SVG_PREVIEW_MIN_DISPLAY_EDGE / 16.).max(1.);
        assert_eq!(
            svg_display_scale(size(px(16.), px(16.)), 1.),
            expected_scale
        );
        assert_eq!(svg_display_scale(size(px(512.), px(256.)), 1.), 1.);
    }

    #[test]
    fn zooming_out_keeps_the_base_raster_resolution() {
        assert_eq!(svg_raster_scale(0.8), 1.);
        assert_eq!(svg_raster_scale(1.), 1.);
        assert_eq!(svg_raster_scale(1.5), 1.5);
    }

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
                PreviewDocument::Source {
                    path: PathBuf::from("icon.svg"),
                    source_item: Box::new(editor),
                    multi_buffer,
                    toggle_preview: Rc::new(|_, _| {}),
                    open_path: None,
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
