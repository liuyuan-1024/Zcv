//! `App` 派发管线的 headless 单元测试。
//!
//! 这一层不接触 GPUI ——只覆盖 keymap 解析、命令派发、IME 流，以及
//! 命令产出的 HostEffect。需要 GPUI 句柄（Entity / Window / 焦点等）的链路
//! 在 `shell::view` 那一层做手工 / 集成测试，不进本文件。

use crate::app::App;
use crate::shell::features::PanelId;
use crate::shell::features::file_tree::FileTreeActivation;
use std::fs::{File, create_dir_all};
use std::path::PathBuf;
use zom_command::HostEffect;
use zom_command::commands::{
    editor, language_server as language_server_commands, workspace as workspace_commands,
};
use zom_workspace::EntryKind;

/// 构造一个已打开项目并激活了一个空文件的 `App`。
///
/// 不再有默认空白 buffer，编辑管线测试必须先真实打开一个文件才有活动 buffer。
/// 复用 `project_fixture`：rows 为 `[root, src, README.md]`，走到 README.md
/// 并 activate。
fn app_with_open_file(name: &str) -> App {
    let mut app = App::new();
    app.open_local_project(project_fixture(name));
    app.file_tree_move_selection(1); // root
    app.file_tree_move_selection(1); // src
    app.file_tree_move_selection(1); // README.md
    assert_eq!(app.file_tree_activate(), FileTreeActivation::OpenedFile);
    app
}

#[test]
fn ime_and_key_input_should_drive_active_buffer_through_command_pipeline() {
    let mut app = app_with_open_file("ime-key");

    // 普通文本输入走 IME 通道（系统输入法或键盘的 NSTextInputClient 提交）。
    app.ime_replace_text(None, "h").unwrap();
    app.ime_replace_text(None, "i").unwrap();

    let state = app.editor_state();
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

    let state = app.editor_state();
    assert_eq!(state.text, "i");
    assert_eq!(state.cursor_byte, 0);

    let outcome = app.dispatch_key_input("mod-z".to_string()).unwrap();
    assert!(outcome.consumed);

    // 没绑定的字符必须返回未消费，让 IME 路径接管。
    assert!(!app.dispatch_key_input("a".to_string()).unwrap().consumed);

    let state = app.editor_state();
    assert_eq!(state.text, "hi");
    assert_eq!(state.cursor_byte, 1);
}

#[test]
fn ime_preedit_update_and_commit_should_flow_through_engine() {
    let mut app = app_with_open_file("ime-preedit");

    // 先输入一个英文字符，确认 IME commit 走单独路径。
    app.ime_replace_text(None, "x").unwrap();

    // 模拟输入法 preedit：先 mark "ni"，再 mark "你"，最后 commit "你"。
    app.ime_replace_and_mark_text(None, "ni", Some(2..2))
        .unwrap();
    let state = app.editor_state();
    assert_eq!(state.text, "xni");
    assert!(app.ime_marked_range_utf16().is_some());

    app.ime_replace_and_mark_text(None, "你", Some(1..1))
        .unwrap();
    let state = app.editor_state();
    assert_eq!(state.text, "x你");

    app.ime_replace_text(None, "你").unwrap();
    let state = app.editor_state();
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
    let mut app = app_with_open_file("tab-enter");

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

    let state = app.editor_state();
    assert_eq!(state.text, "    \n\n");
    assert_eq!(state.cursor_byte, 6);
    assert!(state.dirty);
}

#[test]
fn panel_toggle_command_should_emit_host_effect() {
    let mut app = App::new();

    // 命中 mod-shift-e → editor 区按下时应被 keymap 消费。
    let outcome = app
        .dispatch_key_input("mod-shift-e".to_string())
        .expect("派发成功");
    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![HostEffect::TogglePanel("file_tree".to_string())]
    );
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
    assert_eq!(outcome.effects, vec![HostEffect::ShowProjectPicker]);
}

#[test]
fn open_local_project_command_should_emit_window_action() {
    let mut app = App::new();

    let actions = app
        .dispatch(workspace_commands::open_local_project())
        .unwrap();

    assert_eq!(actions, vec![HostEffect::OpenLocalProject]);
}

#[test]
fn project_title_should_prompt_when_no_project_is_open() {
    let app = App::new();

    assert_eq!(app.project_title(), "打开项目");
}

#[test]
fn open_local_project_should_update_project_title_and_reset_workspace() {
    let mut app = app_with_open_file("reset");
    app.ime_replace_text(None, "临时内容").unwrap();
    assert!(!app.editor_state().text.is_empty());

    app.open_local_project(PathBuf::from("/tmp/zom-local-project"));

    assert_eq!(app.project_title(), "zom-local-project");
    // 重开项目后工作区清空：没有默认 buffer，也没有活动视图。
    let state = app.editor_state();
    assert!(state.title.is_empty());
    assert!(state.text.is_empty());
    assert!(!state.dirty);
}

#[test]
fn language_server_status_command_should_emit_open_overlay_window_action() {
    let mut app = App::new();

    let actions = app
        .dispatch(language_server_commands::open_status())
        .unwrap();

    assert_eq!(actions, vec![HostEffect::ShowLanguageServers]);
}

#[test]
fn escape_should_dispatch_overlay_dismiss_command() {
    let mut app = App::new();

    let outcome = app.dispatch_key_input("escape".to_string()).unwrap();

    assert!(outcome.consumed);
    assert_eq!(outcome.effects, vec![HostEffect::DismissOverlay]);
}

fn project_fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zom-file-tree-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    create_dir_all(dir.join("src/inner")).unwrap();
    File::create(dir.join("README.md")).unwrap();
    File::create(dir.join("src/lib.rs")).unwrap();
    File::create(dir.join("src/inner/mod.rs")).unwrap();
    dir
}

#[test]
fn file_tree_move_selection_should_walk_visible_rows_in_order() {
    let mut app = App::new();
    app.open_local_project(project_fixture("move"));

    assert!(app.file_tree_state().selected.is_none());

    // rows: [root, src, README.md]
    app.file_tree_move_selection(1);
    let state = app.file_tree_state();
    assert_eq!(state.selected.as_ref(), Some(&state.rows[0].path));

    app.file_tree_move_selection(1);
    let state = app.file_tree_state();
    assert_eq!(state.selected.as_ref(), Some(&state.rows[1].path));

    app.file_tree_move_selection(1);
    let state = app.file_tree_state();
    assert_eq!(state.selected.as_ref(), Some(&state.rows[2].path));

    // 已在末位时再 down 不会越界。
    app.file_tree_move_selection(1);
    let state = app.file_tree_state();
    assert_eq!(state.selected.as_ref(), Some(&state.rows[2].path));
}

#[test]
fn file_tree_focus_initialization_should_select_first_visible_row() {
    let mut app = App::new();
    app.open_local_project(project_fixture("focus-init"));

    app.file_tree_ensure_selection_initialized();

    let state = app.file_tree_state();
    assert_eq!(state.selected.as_ref(), Some(&state.rows[0].path));
}

#[test]
fn file_tree_expand_then_collapse_should_round_trip_via_selection_keys() {
    let mut app = App::new();
    let root = project_fixture("expand");
    app.open_local_project(root.clone());

    // 初始 rows: [root, src, README.md]，根默认展开。
    let state = app.file_tree_state();
    assert_eq!(state.rows.len(), 3);

    // 选到 src（root → src）。
    app.file_tree_move_selection(1);
    app.file_tree_move_selection(1);
    assert_eq!(
        app.file_tree_state().selected.as_deref(),
        Some(root.join("src").as_path())
    );

    app.file_tree_expand_or_into();
    let state = app.file_tree_state();
    // 展开 src 后 rows: [root, src, inner, lib.rs, README.md]
    assert_eq!(state.rows.len(), 5);
    assert!(
        state
            .rows
            .iter()
            .find(|r| r.path == root.join("src"))
            .map(|r| r.expanded)
            .unwrap_or(false)
    );

    app.file_tree_collapse_or_parent();
    assert_eq!(app.file_tree_state().rows.len(), 3);
}

#[test]
fn file_tree_activate_on_file_should_open_buffer_and_report_opened() {
    let mut app = App::new();
    let root = project_fixture("activate");
    app.open_local_project(root.clone());

    // rows: [root, src, README.md] —— 走到 README.md。
    app.file_tree_move_selection(1); // root
    app.file_tree_move_selection(1); // src
    app.file_tree_move_selection(1); // README.md
    let selected = app.file_tree_state().selected.clone();
    assert_eq!(selected.as_deref(), Some(root.join("README.md").as_path()));

    let action = app.file_tree_activate();
    assert_eq!(action, FileTreeActivation::OpenedFile);

    let state = app.file_tree_state();
    assert_eq!(
        state.active.as_deref(),
        Some(root.join("README.md").as_path())
    );
}

#[test]
fn file_tree_activate_on_directory_should_toggle_expanded() {
    let mut app = App::new();
    let root = project_fixture("activate-dir");
    app.open_local_project(root.clone());

    // rows: [root, src, README.md] —— 选到 src。
    app.file_tree_move_selection(1); // root
    app.file_tree_move_selection(1); // src
    let action = app.file_tree_activate();
    assert_eq!(action, FileTreeActivation::ToggledDir);

    let state = app.file_tree_state();
    let src_row = state
        .rows
        .iter()
        .find(|r| r.path == root.join("src"))
        .unwrap();
    assert!(matches!(src_row.kind, EntryKind::Directory));
    assert!(src_row.expanded);
}
