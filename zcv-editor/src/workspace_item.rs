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
            EditorEvent::DirtyChanged => emit(ItemEvent::UpdateTab),
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
        // 根相对化查询宿主注册的活动项目根（RootChanged 时由装配层更新）。
        let relative = cx
            .try_global::<zcv_project::ActiveProjectRoot>()
            .and_then(|root| root.0.as_deref())
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(&path);
        Some((vec![relative.to_string_lossy().into_owned().into()], None))
    }

    fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        let Some(path) = self.file_path(cx) else {
            return;
        };
        let Ok(suffix) = path.strip_prefix(from) else {
            return;
        };
        let renamed_path = to.join(suffix);
        self.set_file_path(renamed_path, cx);
    }

    fn buffer(&self, _cx: &App) -> Option<Entity<Buffer>> {
        Some(self.buffer())
    }

    fn as_searchable(
        &self,
        self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn zcv_workspace::SearchableItemHandle>> {
        Some(Box::new(self_handle.clone()))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{AppContext as _, TestAppContext};

    use super::*;

    /// 编辑与路径变化必须映射为对应的 ItemEvent，Pane 依赖它们刷新标签与提升临时标签。
    #[test]
    fn item_events_follow_zed_semantics() {
        let mut events = Vec::new();
        Editor::to_item_events(&EditorEvent::Edited, &mut |event| events.push(event));
        assert_eq!(events, vec![ItemEvent::Edit]);

        events.clear();
        Editor::to_item_events(&EditorEvent::DirtyChanged, &mut |event| events.push(event));
        assert_eq!(events, vec![ItemEvent::UpdateTab]);

        events.clear();
        Editor::to_item_events(&EditorEvent::PathChanged, &mut |event| events.push(event));
        assert_eq!(
            events,
            vec![ItemEvent::UpdateTab, ItemEvent::UpdateBreadcrumbs]
        );
    }

    #[gpui::test]
    fn editor_emits_dirty_changes_after_edit_and_save(cx: &mut TestAppContext) {
        let editor = cx.new(Editor::single_line);
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = Rc::clone(&events);
        let _subscription = cx.update(|cx| {
            cx.subscribe(&editor, move |_, event: &EditorEvent, _| {
                observed.borrow_mut().push(event.clone());
            })
        });

        cx.update_entity(&editor, |editor, cx| editor.set_text("未保存", cx));
        cx.run_until_parked();
        assert!(events.borrow().contains(&EditorEvent::DirtyChanged));

        events.borrow_mut().clear();
        let buffer = cx.read_entity(&editor, |editor, _| editor.buffer());
        cx.update_entity(&buffer, |buffer, cx| {
            buffer.mark_saved();
            cx.notify();
        });
        cx.run_until_parked();
        assert!(events.borrow().contains(&EditorEvent::DirtyChanged));
    }
}
