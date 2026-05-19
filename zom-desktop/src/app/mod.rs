//! app —— 组合根（手册 2 / 13）。
//!
//! P2 最小编辑闭环已接入：组合根持有 `CommandRegistry`、`Keymap`、
//! `Workspace` 与 `ViewSet`，并把输入统一收敛到 command 管线。
//!
//! 依赖方向（手册 2.4）：`app` 可以 import `shell`；`shell` 不可反向 import `app`。

mod command;
mod ime;

use std::path::{Path, PathBuf};

use zom_command::commands::{
    editor, language_server as language_server_commands, overlay as overlay_commands,
    panels as panel_commands, window as window_commands, workspace as workspace_commands,
};
use zom_command::{CommandExecutor, CommandQueue, CommandRegistry, Keymap};
use zom_view::ViewSet;
use zom_workspace::Workspace;

use crate::shell::features::file_tree::{FileTreeActivation, FileTreeModel, FileTreeState};

/// 主编辑区当前可显示的活动 buffer 摘要。
#[derive(Clone, Debug, Default)]
pub(crate) struct EditorState {
    pub(crate) title: String,
    pub(crate) text: String,
    pub(crate) cursor_byte: usize,
    pub(crate) dirty: bool,
}

pub struct App {
    registry: CommandRegistry,
    keymap: Keymap,
    executor: CommandExecutor,
    queue: CommandQueue,
    workspace: Workspace,
    views: ViewSet,
    project_root: Option<PathBuf>,
    file_tree: FileTreeModel,
}

impl App {
    pub fn new() -> Self {
        let mut registry = CommandRegistry::new();
        let mut keymap = Keymap::new();

        // 全部命令 + 默认键位都集中在 zom-command::commands 里声明，组合根只
        // 选要装哪些 catalog。handler 看不到的宿主侧资源（窗口、Dock）走
        // HostEffect 反馈到 `apply_host_effect`。
        editor::install(&mut registry, &mut keymap);
        overlay_commands::install(&mut registry, &mut keymap);
        language_server_commands::install(&mut registry, &mut keymap);
        workspace_commands::install(&mut registry, &mut keymap);
        window_commands::install(&mut registry, &mut keymap);
        panel_commands::install(&mut registry, &mut keymap);

        let (workspace, views) = empty_workspace();

        Self {
            registry,
            keymap,
            executor: CommandExecutor::new(),
            queue: CommandQueue::new(),
            workspace,
            views,
            project_root: None,
            file_tree: FileTreeModel::default(),
        }
    }

    pub(crate) fn open_local_project(&mut self, root: PathBuf) {
        self.file_tree.open_project(root.clone());
        self.project_root = Some(root);
        let (workspace, views) = empty_workspace();
        self.workspace = workspace;
        self.views = views;
    }

    pub(crate) fn project_title(&self) -> String {
        self.project_root
            .as_deref()
            .and_then(project_name)
            .unwrap_or("打开项目")
            .to_string()
    }

    pub(crate) fn has_project(&self) -> bool {
        self.project_root.is_some()
    }

    pub(crate) fn file_tree_state(&self) -> FileTreeState {
        self.file_tree.state(&self.workspace)
    }

    pub(crate) fn file_tree_ensure_selection_initialized(&mut self) {
        self.file_tree.ensure_selection_initialized();
    }

    pub(crate) fn file_tree_move_selection(&mut self, delta: isize) {
        self.file_tree.move_selection(delta);
    }

    pub(crate) fn file_tree_collapse_or_parent(&mut self) {
        self.file_tree.collapse_or_parent();
    }

    pub(crate) fn file_tree_expand_or_into(&mut self) {
        self.file_tree.expand_or_into();
    }

    pub(crate) fn file_tree_activate(&mut self) -> FileTreeActivation {
        self.file_tree
            .activate_selected(&mut self.workspace, &mut self.views)
    }

    pub(crate) fn editor_state(&self) -> EditorState {
        let Some(view) = self.views.active_view() else {
            return EditorState::default();
        };

        let buffer_id = view.buffer();
        let Some(buffer) = self.workspace.buffer(buffer_id) else {
            return EditorState::default();
        };

        let title = buffer
            .path()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未命名".to_string());
        let text = buffer.buffer().text().into_owned();
        let cursor_byte = view.selection().primary().head().get();

        EditorState {
            title,
            text,
            cursor_byte,
            dirty: buffer.is_dirty(),
        }
    }
}

fn empty_workspace() -> (Workspace, ViewSet) {
    let mut workspace = Workspace::new();
    let buffer_id = workspace
        .open_text(None, "")
        .expect("默认空白 buffer 必须能创建");
    let base_version = workspace
        .buffer(buffer_id)
        .expect("刚创建的 buffer 必须存在")
        .buffer()
        .version();
    let mut views = ViewSet::new();
    views.open_view(buffer_id, base_version);
    (workspace, views)
}

fn project_name(path: &Path) -> Option<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
