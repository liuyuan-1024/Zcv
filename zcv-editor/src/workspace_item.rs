//! Editor 的 Workspace Item 能力。

use std::path::{Path, PathBuf};

use gpui::{App, Context, Entity, SharedString};
use zcv_engine::Buffer;
use zcv_workspace::{Item, ItemEvent, ToolbarItemLocation};

use crate::{Editor, EditorEvent};

impl Item for Editor {
    type Event = EditorEvent;

    fn tab_content_text(&self, _detail: usize, cx: &App) -> SharedString {
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
            EditorEvent::PathChanged => {
                // 路径变化同时刷新标签标题与面包屑。
                emit(ItemEvent::UpdateTab);
                emit(ItemEvent::UpdateBreadcrumbs);
            }
            EditorEvent::Edited => emit(ItemEvent::Edit),
        }
    }

    fn can_save(&self, cx: &App) -> bool {
        self.file_path(cx).is_some()
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 编辑与路径变化必须映射为对应的 ItemEvent，Pane 依赖它们刷新标签与提升临时标签。
    #[test]
    fn item_events_follow_zed_semantics() {
        let mut events = Vec::new();
        Editor::to_item_events(&EditorEvent::Edited, &mut |event| events.push(event));
        assert_eq!(events, vec![ItemEvent::Edit]);

        events.clear();
        Editor::to_item_events(&EditorEvent::PathChanged, &mut |event| events.push(event));
        assert_eq!(
            events,
            vec![ItemEvent::UpdateTab, ItemEvent::UpdateBreadcrumbs]
        );
    }
}
