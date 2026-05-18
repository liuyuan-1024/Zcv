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
    editor, panels as panel_commands, window as window_commands, workspace as workspace_commands,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::model::PanelId;
    use zom_command::commands::workspace as workspace_commands;

    #[test]
    fn ime_and_key_input_should_drive_active_buffer_through_command_pipeline() {
        let mut app = App::new();

        // 普通文本输入走 IME 通道（系统输入法或键盘的 NSTextInputClient 提交）。
        app.ime_replace_text(None, "h").unwrap();
        app.ime_replace_text(None, "i").unwrap();

        let state = app.workbench_state().editor;
        assert_eq!(state.text, "hi");
        assert_eq!(state.cursor_byte, 2);
        assert!(state.dirty);

        // 非文本按键仍走 keymap → 命令。
        assert!(app.dispatch_key_input("left".to_string()).unwrap().consumed);
        assert!(
            app.dispatch_key_input("backspace".to_string())
                .unwrap()
                .consumed
        );

        let state = app.workbench_state().editor;
        assert_eq!(state.text, "i");
        assert_eq!(state.cursor_byte, 0);

        let outcome = app.dispatch_key_input("mod-z".to_string()).unwrap();
        assert!(outcome.consumed);

        // 没绑定的字符必须返回未消费，让 IME 路径接管。
        assert!(!app.dispatch_key_input("a".to_string()).unwrap().consumed);

        let state = app.workbench_state().editor;
        assert_eq!(state.text, "hi");
        assert_eq!(state.cursor_byte, 1);
    }

    #[test]
    fn ime_preedit_update_and_commit_should_flow_through_engine() {
        let mut app = App::new();

        // 先输入一个英文字符，确认 IME commit 走单独路径。
        app.ime_replace_text(None, "x").unwrap();

        // 模拟输入法 preedit：先 mark "ni"，再 mark "你"，最后 commit "你"。
        app.ime_replace_and_mark_text(None, "ni", Some(2..2))
            .unwrap();
        let state = app.workbench_state().editor;
        assert_eq!(state.text, "xni");
        assert!(app.ime_marked_range_utf16().is_some());

        app.ime_replace_and_mark_text(None, "你", Some(1..1))
            .unwrap();
        let state = app.workbench_state().editor;
        assert_eq!(state.text, "x你");

        app.ime_replace_text(None, "你").unwrap();
        let state = app.workbench_state().editor;
        assert_eq!(state.text, "x你");
        assert!(app.ime_marked_range_utf16().is_none());
        // commit 之后 cursor 落在 "你" 之后，对应 4 个 UTF-8 字节 + 1 (x)。
        assert_eq!(state.cursor_byte, 1 + "你".len());

        // selected_range_utf16 用 UTF-16 计数：x 占 1，你 占 1，总长 2。
        let (sel, _) = app.ime_selected_range_utf16().unwrap();
        assert_eq!(sel, 2..2);
    }

    #[test]
    fn tab_and_enter_should_dispatch_editor_commands() {
        let mut app = App::new();

        assert!(app.dispatch_key_input("tab".to_string()).unwrap().consumed);
        assert!(
            app.dispatch_key_input("enter".to_string())
                .unwrap()
                .consumed
        );
        assert!(
            app.dispatch_key_input("return".to_string())
                .unwrap()
                .consumed
        );

        let state = app.workbench_state().editor;
        assert_eq!(state.text, "    \n\n");
        assert_eq!(state.cursor_byte, 6);
        assert!(state.dirty);
    }

    #[test]
    fn panel_toggle_command_should_drive_dock_visibility_through_binding() {
        let mut app = App::new();
        let initial_visible = app.left_dock.is_visible();
        let file_tree_active = app.left_dock.active_panel() == Some(PanelId::FileTree);

        // 命中 mod-shift-e → editor 区按下时应被 keymap 消费。
        let outcome = app
            .dispatch_key_input("mod-shift-e".to_string())
            .expect("派发成功");
        assert!(outcome.consumed);

        // 初始状态如果文件树已经是 active 且 dock 可见 → 折叠；否则展开 + 切到文件树。
        if initial_visible && file_tree_active {
            assert!(app.left_dock.collapsed);
        } else {
            assert!(!app.left_dock.collapsed);
            assert_eq!(app.left_dock.active_panel(), Some(PanelId::FileTree));
        }

        // 再来一次：应该回到与上一步相反的状态。
        let before = app.left_dock.collapsed;
        app.dispatch_key_input("mod-shift-e".to_string()).unwrap();
        assert_ne!(app.left_dock.collapsed, before);
    }

    #[test]
    fn shortcut_for_should_return_formatted_keymap_binding() {
        let app = App::new();

        // 已绑定的命令：返回格式化后的快捷键。
        let undo = app.shortcut_for(editor::UNDO).expect("undo 必有快捷键");
        let save = app
            .shortcut_for(workspace_commands::SAVE)
            .expect("save 必有快捷键");
        let file_tree = app
            .shortcut_for(PanelId::FileTree.toggle_command_id())
            .expect("file_tree 切换必有快捷键");

        // 平台差异化校验在专门的格式化测试里做；这里只关心"能查到、非空"。
        assert!(!undo.is_empty());
        assert!(!save.is_empty());
        assert!(!file_tree.is_empty());

        // 未注册 / 未绑定的命令：返回 None。
        // settings.open 命令 id 已在 zom-command 占位（commands::settings），
        // 但 catalog 还没 install handler / 绑键，所以反查应当 None。
        assert!(
            app.shortcut_for(zom_command::commands::settings::OPEN)
                .is_none()
        );
        assert!(app.shortcut_for("不存在的命令").is_none());
    }
}
