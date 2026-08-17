//! 文本文件 Item Provider：项目文件 → Editor。
//!
//! 打开文件时框架经 ItemProvider 注册表分发到本 provider，使 Workspace 不直接依赖 Editor 类型（对齐 Zed 的 WorkspaceItemBuilder）。

use std::path::{Path, PathBuf};

use gpui::{App, AppContext, Entity, Task};
use zcv_project::Project;
use zcv_workspace::{ItemHandle, ItemProvider};

use crate::Editor;

/// 任何有扩展名的普通文件都交给编辑器打开（文本兜底）。
pub struct TextFileProvider;

impl ItemProvider for TextFileProvider {
    fn supports(&self, path: &Path, _cx: &App) -> bool {
        path.extension().is_some()
    }

    fn open_item(
        &self,
        path: PathBuf,
        project: Entity<Project>,
        cx: &mut App,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        let root = project.read(cx).root().to_path_buf();
        let buffer = match project.update(cx, |project, cx| project.open_buffer(&path, cx)) {
            Ok(buffer) => buffer,
            Err(error) => return Task::ready(Err(anyhow::anyhow!("{error}"))),
        };
        let editor = cx.new(|cx| Editor::for_buffer(buffer, cx));
        editor.update(cx, |editor, cx| {
            editor.set_file_path(path, root, cx);
        });
        Task::ready(Ok(Box::new(editor) as Box<dyn ItemHandle>))
    }
}

/// 注册文本文件 Provider；可重复调用（按具体 Provider 类型去重）。
pub fn init(cx: &mut App) {
    zcv_workspace::register_item_provider(TextFileProvider, cx);
}
