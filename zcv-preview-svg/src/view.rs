use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyEntity, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Image, ImageFormat,
    IntoElement, ObjectFit, Render, RenderImage, SharedString, Styled, StyledImage, Subscription,
    Task, Window, div, img, prelude::*,
};
use zcv_editor::{Editor, EditorEvent};
use zcv_engine::{Buffer, ByteOffset};
use zcv_preview::PreviewDocument;
use zcv_theme::color;
use zcv_workspace::{DocumentItemKey, Item, ItemEvent, ItemPresentation, ToolbarItemLocation};

use crate::renderer::rasterize_svg;

enum SvgPreviewState {
    Loading,
    Ready(Arc<RenderImage>),
    Error(String),
}

pub struct SvgPreviewView {
    source_editor: Entity<Editor>,
    buffer: Entity<Buffer>,
    resources_dir: Option<PathBuf>,
    focus: FocusHandle,
    state: SvgPreviewState,
    render_generation: u64,
    render_task: Option<Task<()>>,
    _buffer_subscription: Subscription,
    _editor_subscription: Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvgPreviewEvent {
    SourcePathChanged,
}

impl SvgPreviewView {
    pub fn new(document: PreviewDocument, cx: &mut Context<Self>) -> Self {
        let source_editor = document.source_editor;
        let buffer = source_editor.read(cx).buffer();
        let resources_dir = document.path.parent().map(PathBuf::from);
        let buffer_subscription = cx.observe(&buffer, |view, _, cx| {
            view.start_render(cx);
        });
        let editor_subscription =
            cx.subscribe(&source_editor, |view, editor, event, cx| match event {
                EditorEvent::PathChanged => {
                    view.resources_dir = editor
                        .read(cx)
                        .file_path(cx)
                        .and_then(|path| path.parent().map(PathBuf::from));
                    view.start_render(cx);
                    cx.emit(SvgPreviewEvent::SourcePathChanged);
                }
            });
        let mut view = Self {
            source_editor,
            buffer,
            resources_dir,
            focus: cx.focus_handle(),
            state: SvgPreviewState::Loading,
            render_generation: 0,
            render_task: None,
            _buffer_subscription: buffer_subscription,
            _editor_subscription: editor_subscription,
        };
        view.start_render(cx);
        view
    }

    fn start_render(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.buffer.read(cx).snapshot();
        let bytes = match snapshot.slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes()) {
            Ok(slice) => slice.as_str().as_bytes().to_vec(),
            Err(error) => {
                self.state = SvgPreviewState::Error(error.to_string());
                self.render_task = None;
                cx.notify();
                return;
            }
        };
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
                    || view.buffer.read(cx).snapshot().version() != version
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
            .key_context("Preview")
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
        self.source_editor
            .read(cx)
            .file_path(cx)
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
        self.source_editor.read(cx).is_dirty(cx)
    }

    fn file_path(&self, cx: &App) -> Option<PathBuf> {
        self.source_editor.read(cx).file_path(cx)
    }

    fn breadcrumb_location(&self, _cx: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::PrimaryLeft
    }

    fn breadcrumbs(&self, cx: &App) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        let editor = self.source_editor.read(cx);
        let path = editor.file_path(cx)?;
        let relative = editor
            .project_root()
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(&path);
        Some((
            vec![
                relative.to_string_lossy().into_owned().into(),
                "Preview".into(),
            ],
            None,
        ))
    }

    fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        self.source_editor.update(cx, |editor, cx| {
            let (Some(path), Some(project_root)) = (
                editor.file_path(cx),
                editor.project_root().map(Path::to_path_buf),
            ) else {
                return;
            };
            let Ok(suffix) = path.strip_prefix(from) else {
                return;
            };
            let renamed_path = to.join(suffix);
            let renamed_root = project_root
                .strip_prefix(from)
                .map_or(project_root.clone(), |suffix| to.join(suffix));
            editor.set_file_path(renamed_path, renamed_root, cx);
        });
    }

    fn buffer(&self, _cx: &App) -> Option<Entity<Buffer>> {
        Some(self.buffer.clone())
    }

    fn document_item_key(&self, _cx: &App) -> Option<DocumentItemKey> {
        Some(DocumentItemKey {
            buffer_id: self.buffer.entity_id(),
            presentation: ItemPresentation::Preview("svg"),
        })
    }

    fn act_as_type(
        &self,
        type_id: TypeId,
        self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<AnyEntity> {
        if type_id == TypeId::of::<Self>() {
            Some(self_handle.clone().into())
        } else if type_id == TypeId::of::<Editor>() {
            Some(self.source_editor.clone().into())
        } else {
            None
        }
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
            editor.set_file_path(PathBuf::from("icon.svg"), PathBuf::from("."), cx);
        });
        let view = cx.new(|cx| {
            SvgPreviewView::new(
                PreviewDocument {
                    path: PathBuf::from("icon.svg"),
                    source_editor: editor,
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
