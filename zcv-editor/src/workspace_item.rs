//! Editor 的 Workspace Item 能力。

use std::path::{Path, PathBuf};

use gpui::{App, Context, Entity, SharedString};
use zcv_engine::Buffer;
use zcv_workspace::{DocumentItemKey, Item, ItemEvent, ItemPresentation, ToolbarItemLocation};

use crate::{Editor, EditorEvent};

impl Item for Editor {
    type Event = EditorEvent;

    fn tab_content_text(&self, cx: &App) -> SharedString {
        self.file_path(cx)
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default()
            .into()
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        match event {
            EditorEvent::PathChanged => emit(ItemEvent::UpdateBreadcrumbs),
        }
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.is_dirty(cx)
    }

    fn file_path(&self, cx: &App) -> Option<PathBuf> {
        self.file_path(cx)
    }

    fn breadcrumb_location(&self, _cx: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::PrimaryLeft
    }

    fn breadcrumbs(&self, cx: &App) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        let path = self.file_path(cx)?;
        let relative = self
            .project_root()
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(&path);
        Some((vec![relative.to_string_lossy().into_owned().into()], None))
    }

    fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        let (Some(path), Some(project_root)) = (
            self.file_path(cx),
            self.project_root().map(Path::to_path_buf),
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
        self.set_file_path(renamed_path, renamed_root, cx);
    }

    fn buffer(&self, _cx: &App) -> Option<Entity<Buffer>> {
        Some(self.buffer())
    }

    fn document_item_key(&self, _cx: &App) -> Option<DocumentItemKey> {
        Some(DocumentItemKey {
            buffer_id: self.buffer().entity_id(),
            presentation: ItemPresentation::Source,
        })
    }
}
