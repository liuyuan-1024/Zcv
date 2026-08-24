//! Editor 的 Workspace Item 能力。

use std::path::{Path, PathBuf};

use gpui::{App, Context, Entity, SharedString, Task, Window};
use zcv_multi_buffer::MultiBuffer;
use zcv_project::Project;
use zcv_workspace::{Item, ItemEvent, ToolbarItemLocation};

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

    fn item_path(&self, cx: &App) -> Option<PathBuf> {
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
        // 条目自身重命名时后缀为空：直接取 to。
        // `to.join(空路径)` 会追加尾随斜杠，保存这类路径会触发 Not a directory。
        let renamed_path = if suffix.as_os_str().is_empty() {
            to.to_path_buf()
        } else {
            to.join(suffix)
        };
        self.set_file_path(renamed_path, cx);
    }

    fn multi_buffer(&self, _cx: &App) -> Option<Entity<MultiBuffer>> {
        Some(self.multi_buffer())
    }

    fn save(
        &mut self,
        project: Entity<Project>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let Some(path) = self.file_path(cx) else {
            return Task::ready(Ok(()));
        };
        let multi_buffer = self.multi_buffer();
        let result = project.update(cx, |project, cx| {
            project.save_buffer(&multi_buffer, &path, cx)
        });
        Task::ready(result.map_err(|error| anyhow::anyhow!("{error}")))
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
        let buffer = cx.read_entity(&editor, |editor, cx| editor.singleton_buffer(cx));
        cx.update_entity(&buffer, |buffer, cx| {
            buffer.mark_saved();
            cx.notify();
        });
        cx.run_until_parked();
        assert!(events.borrow().contains(&EditorEvent::DirtyChanged));
    }

    /// 复现「重命名后保存失败」：重命名必须同步 Editor 的路径，否则保存会落到旧路径（旧路径已不存在 → IO 失败）。
    #[gpui::test]
    fn rename_then_save_writes_to_new_path(cx: &mut TestAppContext) {
        use std::fs;
        use zcv_project::Project;
        use zcv_workspace::ItemHandle;

        // 项目根与打开路径统一走 canonical 形式（与真实装配一致）。
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let root = directory.path().canonicalize().expect("临时目录应可规范化");
        let old_path = root.join("foo.rs");
        let new_path = root.join("bar.rs");
        fs::write(&old_path, "旧内容").expect("应创建测试文件");

        let project = cx.new(|cx| Project::new(root, cx));
        // 对齐 ItemProvider 打开流程：open_buffer 返回已承载规范路径的 MultiBuffer。
        let editor = project.update(cx, |project, cx| {
            let multi_buffer = project.open_buffer(&old_path, cx).expect("应打开测试文件");
            cx.new(|cx| Editor::for_multi_buffer(multi_buffer, cx))
        });

        // 对齐 Workspace::rename_path：项目先迁移，再逐个 item 迁移路径（走真实 ItemHandle 实现）。
        project
            .update(cx, |project, cx| {
                project.rename_path(&old_path, &new_path, cx)
            })
            .expect("应重命名文件");
        let item: Box<dyn ItemHandle> = Box::new(editor.clone());
        cx.update(|cx| item.rename_path(&old_path, &new_path, cx));
        let path = cx.read_entity(&editor, |editor, cx| {
            editor.file_path(cx).expect("重命名后应有路径")
        });
        assert_eq!(path, new_path, "编辑器路径应迁移到新路径");
        // 保存路径不得带尾随斜杠：join 空后缀生成的 `new_path/` 会导致 Not a directory。
        assert_eq!(path.file_name(), Some(std::ffi::OsStr::new("bar.rs")));

        // 编辑后保存：应写入新路径。
        cx.update_entity(&editor, |editor, cx| editor.set_text("新内容", cx));
        let multi_buffer = cx.read_entity(&editor, |editor, _| editor.multi_buffer());
        project
            .update(cx, |project, cx| {
                project.save_buffer(&multi_buffer, &path, cx)
            })
            .expect("保存应成功");
        assert_eq!(
            fs::read_to_string(&new_path).expect("新路径应有保存内容"),
            "新内容"
        );
        assert!(!old_path.exists(), "旧路径不应再存在");
    }
}
