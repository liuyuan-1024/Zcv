//! Editor 的 Workspace Item 能力。

use std::path::{Path, PathBuf};

use gpui::{App, Context, Entity, SharedString, Task, Window};
use zcv_multi_buffer::MultiBuffer;
use zcv_path::simplify_native;
use zcv_project::Project;
use zcv_workspace::{Item, ItemEvent, SearchableItemHandle};

use crate::view::NAVIGATION_TOP_OFFSET;
use crate::{Editor, EditorEvent};

impl Item for Editor {
    type Event = EditorEvent;

    fn tab_content_text(&self, cx: &App) -> SharedString {
        // 标题归文档模型所有：显式标题优先，否则按文档身份派生。
        self.multi_buffer()
            .read(cx)
            .title(cx)
            .unwrap_or_default()
            .into()
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        match event {
            EditorEvent::PathChanged => emit(ItemEvent::PathChanged),
            EditorEvent::DirtyChanged => emit(ItemEvent::UpdateTab),
            EditorEvent::Edited { .. } => emit(ItemEvent::Edit),
            EditorEvent::OpenExcerptsRequested { .. } => {}
            EditorEvent::DiffHunksExpandedChanged => {}
            EditorEvent::Error(message) => emit(ItemEvent::Error(message.clone())),
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
        // 文件系统事件可能携带 Windows 平台扩展路径，而编辑器文档身份使用普通路径。
        // 在匹配路径前统一表示形式，避免同一文件因 `\\?\` 前缀无法迁移。
        let from = simplify_native(from);
        let to = simplify_native(to);
        let Ok(suffix) = path.strip_prefix(&from) else {
            return;
        };
        // 条目自身重命名时后缀为空：直接取 to。
        // `to.join(空路径)` 会追加尾随斜杠，保存这类路径会触发 Not a directory。
        let renamed_path = if suffix.as_os_str().is_empty() {
            to
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
        if range.end
            > self
                .multi_buffer()
                .update(cx, |buffer, cx| buffer.snapshot(cx))
                .len_bytes()
                .get()
        {
            return false;
        }
        self.select_byte_range(range, cx);
        // 导航定位：目标行固定在视口顶部下方，避免落在视口内的随机位置。
        self.request_scroll_to_top(NAVIGATION_TOP_OFFSET, cx);
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
    ) -> Option<Box<dyn SearchableItemHandle>> {
        Some(Box::new(self_handle.clone()))
    }
}

#[cfg(test)]
#[path = "test/workspace_item_tests.rs"]
mod tests;
