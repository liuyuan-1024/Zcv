//! Editor 的 Workspace Item 能力。

use std::path::{Path, PathBuf};

use gpui::{App, Context, Entity, SharedString, Task, Window};
use zcv_multi_buffer::MultiBuffer;
use zcv_project::Project;
use zcv_workspace::{Item, ItemEvent, ToolbarItemLocation};

use crate::view::NAVIGATION_TOP_OFFSET;
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
            EditorEvent::PathChanged => emit(ItemEvent::PathChanged),
            EditorEvent::DirtyChanged => emit(ItemEvent::UpdateTab),
            EditorEvent::Edited => emit(ItemEvent::Edit),
            EditorEvent::OpenExcerptsRequested { .. } => {}
            EditorEvent::DiffHunksExpandedChanged => {}
        }
    }

    fn can_save(&self, cx: &App) -> bool {
        !self.multi_buffer().read(cx).file_buffers(cx).is_empty()
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

    fn breadcrumbs(
        &self,
        project_root: Option<&Path>,
        cx: &App,
    ) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        let path = self.file_path(cx)?;
        let relative = project_root
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

    fn navigate_to_byte_range(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> bool {
        if range.end > self.text_buffer(cx).read(cx).len_bytes().get() {
            return false;
        }
        self.select_byte_range(range, cx);
        // 导航定位：目标行固定在视口顶部下方，避免落在视口内的随机位置。
        self.request_scroll_to_top(NAVIGATION_TOP_OFFSET);
        true
    }

    fn navigate_to_line_column(
        &mut self,
        line: usize,
        column: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        Editor::navigate_to_line_column(self, line, column, cx)
    }

    fn save(
        &mut self,
        project: Entity<Project>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let buffers = self
            .multi_buffer()
            .read(cx)
            .file_buffers(cx)
            .into_iter()
            .filter(|(buffer, _)| buffer.read(cx).is_dirty())
            .collect::<Vec<_>>();
        let result = project.update(cx, |project, cx| project.save_file_buffers(buffers, cx));
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
    use std::fs;
    use std::rc::Rc;

    use gpui::{AppContext as _, Empty, TestAppContext};
    use zcv_multi_buffer::MultiBufferExcerpt;
    use zcv_text::{ByteOffset, TextRange};
    use zcv_workspace::ItemHandle;

    use super::*;

    /// 编辑、脏状态与路径变化必须映射为各自的 ItemEvent。
    #[test]
    fn item_events_preserve_distinct_semantics() {
        let mut events = Vec::new();
        Editor::to_item_events(&EditorEvent::Edited, &mut |event| events.push(event));
        assert_eq!(events, vec![ItemEvent::Edit]);

        events.clear();
        Editor::to_item_events(&EditorEvent::DirtyChanged, &mut |event| events.push(event));
        assert_eq!(events, vec![ItemEvent::UpdateTab]);

        events.clear();
        Editor::to_item_events(&EditorEvent::PathChanged, &mut |event| events.push(event));
        assert_eq!(events, vec![ItemEvent::PathChanged]);
    }

    #[gpui::test]
    fn breadcrumbs_use_the_supplied_project_root(cx: &mut TestAppContext) {
        let editor = cx.new(Editor::single_line);
        cx.update_entity(&editor, |editor, cx| {
            editor.set_file_path(PathBuf::from("/project/src/main.rs"), cx);
        });

        let breadcrumbs = cx.read_entity(&editor, |editor, cx| {
            Item::breadcrumbs(editor, Some(Path::new("/project")), cx)
        });
        let segments = breadcrumbs.expect("编辑器应提供面包屑").0;
        assert_eq!(segments.as_slice(), &[SharedString::from("src/main.rs")]);
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
        // dirty 的权威来源是工作区源 Buffer；
        // 投影 Buffer 只由组合文档重建，不参与保存状态。
        let buffer = cx.read_entity(&editor, |editor, cx| {
            editor
                .multi_buffer()
                .read(cx)
                .as_singleton(cx)
                .expect("单行编辑器的整文件源应可取回")
        });
        cx.update_entity(&buffer, |buffer, cx| {
            buffer.mark_saved();
            cx.notify();
        });
        cx.run_until_parked();
        assert!(events.borrow().contains(&EditorEvent::DirtyChanged));
    }

    #[gpui::test]
    fn composite_editor_saves_dirty_source_files(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let root = directory.path().canonicalize().expect("临时目录应可规范化");
        let path = root.join("source.txt");
        std::fs::write(&path, "旧内容\n").expect("应创建源文件");
        let project = cx.new(|cx| Project::new(root, cx));
        let source = project.update(cx, |project, cx| {
            project.open_buffer(&path, cx).expect("应打开源文件")
        });
        let source_len = cx.read_entity(&source, |source, cx| source.text_snapshot(cx).len_bytes());
        let combined = cx.new(MultiBuffer::empty);
        combined.update(cx, |combined, cx| {
            combined.set_excerpts(
                vec![MultiBufferExcerpt::new(
                    source,
                    TextRange::new(ByteOffset::ZERO, source_len).expect("完整范围应有效"),
                    Vec::new(),
                )],
                cx,
            )
        });
        let editor = cx.new(|cx| Editor::for_multi_buffer(combined, cx));
        editor.update(cx, |editor, cx| editor.set_text("新内容\n", cx));

        let item: Box<dyn zcv_workspace::ItemHandle> = Box::new(editor.clone());
        cx.update(|cx| {
            assert!(item.can_save(cx));
            assert!(item.is_dirty(cx));
        });
        let window = cx.add_window(|_, _| Empty);
        let _save = window
            .update(cx, |_, window, cx| item.save(project, window, cx))
            .expect("测试窗口应保持可用");

        assert_eq!(
            std::fs::read_to_string(path).expect("应读取已保存文件"),
            "新内容\n"
        );
        cx.update(|cx| assert!(!item.is_dirty(cx)));
    }

    /// 复现「重命名后保存失败」：重命名必须同步 Editor 的路径，否则保存会落到旧路径（旧路径已不存在 → IO 失败）。
    #[gpui::test]
    fn rename_then_save_writes_to_new_path(cx: &mut TestAppContext) {
        // 项目根与打开路径统一走 canonical 形式（与真实装配一致）。
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let root = directory.path().canonicalize().expect("临时目录应可规范化");
        let old_path = root.join("foo.rs");
        let new_path = root.join("bar.rs");
        fs::write(&old_path, "旧内容").expect("应创建测试文件");

        let project = cx.new(|cx| Project::new(root, cx));
        // open_buffer 返回已承载规范路径的 LanguageBuffer（与 item_provider 同路径包装成组合文档）。
        let language_buffer = project.update(cx, |project, cx| {
            project.open_buffer(&old_path, cx).expect("应打开测试文件")
        });
        let editor = cx.new(|cx| {
            let multi_buffer = cx.new(|cx| MultiBuffer::from_working_source(language_buffer, cx));
            Editor::for_multi_buffer(multi_buffer, cx)
        });

        // 项目先迁移，再逐个 item 迁移路径（走真实 ItemHandle 实现）。
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
