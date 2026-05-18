//! app —— 组合根（手册 2 / 13）。
//!
//! P2 最小编辑闭环已接入：组合根持有窗口级布局状态、`CommandRegistry`、
//! `Keymap`、`Workspace` 与 `ViewSet`，并把输入统一收敛到 command 管线。
//!
//! 依赖方向（手册 2.4）：`app` 可以 import `shell`；`shell` 不可反向 import `app`。

mod command;
mod default_layout;
mod ime;

use crate::shell::model::{BottomBarState, DockState, EditorState, WorkbenchState};

use zom_command::commands::{
    editor, language_server as language_server_commands, overlay as overlay_commands,
    panels as panel_commands, window as window_commands, workspace as workspace_commands,
};
use zom_command::{CommandExecutor, CommandQueue, CommandRegistry, Keymap};
use zom_view::ViewSet;
use zom_workspace::Workspace;

pub struct App {
    left_dock: DockState,
    right_dock: DockState,
    bottom_dock: DockState,
    bottom_bar: BottomBarState,
    registry: CommandRegistry,
    keymap: Keymap,
    executor: CommandExecutor,
    queue: CommandQueue,
    workspace: Workspace,
    views: ViewSet,
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

        Self {
            left_dock: default_layout::default_left_dock(),
            right_dock: default_layout::default_right_dock(),
            bottom_dock: default_layout::default_bottom_dock(),
            bottom_bar: BottomBarState::default(),
            registry,
            keymap,
            executor: CommandExecutor::new(),
            queue: CommandQueue::new(),
            workspace,
            views,
        }
    }

    /// 把当前 App 状态投影为 shell 渲染所需的 `WorkbenchState`。
    ///
    /// 骨架阶段直接克隆；将来 dock 状态升级为 `Entity<DockState>` 后，本方法
    /// 改为返回轻量引用 / 句柄包，避免每帧 clone。
    pub(crate) fn workbench_state(&self) -> WorkbenchState {
        WorkbenchState {
            left_dock: self.left_dock.clone(),
            right_dock: self.right_dock.clone(),
            bottom_dock: self.bottom_dock.clone(),
            bottom_bar: self.bottom_bar.clone(),
            editor: self.editor_state(),
        }
    }

    fn editor_state(&self) -> EditorState {
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

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
