//! `App` 派发管线的 headless 单元测试。
//!
//! 这一层不接触 GPUI ——只覆盖 keymap 解析、命令派发、IME 流，以及
//! HostEffect → `WindowAction` 翻译。需要 GPUI 句柄（Entity / Window / 焦点
//! 等）的链路在 `shell::view` 那一层做手工 / 集成测试，不进本文件。

use crate::app::App;
use crate::shell::model::PanelId;
use crate::shell::overlay::OverlayKind;
use crate::shell::platform::window::WindowAction;
use zom_command::commands::{editor, workspace as workspace_commands};

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
    let initial = app.workbench_state().left_dock;
    let initial_visible = initial.is_visible();
    let file_tree_active = initial.active_panel() == Some(PanelId::FileTree);

    // 命中 mod-shift-e → editor 区按下时应被 keymap 消费。
    let outcome = app
        .dispatch_key_input("mod-shift-e".to_string())
        .expect("派发成功");
    assert!(outcome.consumed);

    // 初始状态如果文件树已经是 active 且 dock 可见 → 折叠；否则展开 + 切到文件树。
    let after_first = app.workbench_state().left_dock;
    if initial_visible && file_tree_active {
        assert!(after_first.collapsed);
    } else {
        assert!(!after_first.collapsed);
        assert_eq!(after_first.active_panel(), Some(PanelId::FileTree));
    }

    // 再来一次：应该回到与上一步相反的状态。
    let before = after_first.collapsed;
    app.dispatch_key_input("mod-shift-e".to_string()).unwrap();
    assert_ne!(app.workbench_state().left_dock.collapsed, before);
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

#[test]
fn project_picker_command_should_emit_open_overlay_window_action() {
    let mut app = App::new();

    let outcome = app.dispatch_key_input("mod-o".to_string()).unwrap();

    assert!(outcome.consumed);
    assert_eq!(
        outcome.actions,
        vec![WindowAction::OpenOverlay(OverlayKind::ProjectPicker)]
    );
}

#[test]
fn escape_should_dispatch_overlay_dismiss_command() {
    let mut app = App::new();

    let outcome = app.dispatch_key_input("escape".to_string()).unwrap();

    assert!(outcome.consumed);
    assert_eq!(outcome.actions, vec![WindowAction::DismissOverlay]);
}
