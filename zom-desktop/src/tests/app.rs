//! `App` 派发管线的 headless 单元测试。
//!
//! 这一层不接触 GPUI ——只覆盖 keymap 解析、命令派发、IME 流，以及
//! 命令产出的 HostEffect。需要 GPUI 句柄（Entity / Window / 焦点等）的链路
//! 在 `shell::view` 那一层做手工 / 集成测试，不进本文件。

use crate::app::{App, EditorState, EditorTab, KeySurface};
use crate::shell::features::PanelId;
use crate::shell::features::file_tree::FileTreeActivation;
use std::fs::{File, create_dir_all};
use std::path::PathBuf;
use zom_command::HostEffect;
use zom_command::commands::{
    diagnostics, editor, language_servers, project_picker as project_picker_commands, settings,
};
use zom_workspace::EntryKind;

/// 取当前活动标签——断言「编辑区正在显示哪个文件」用。
fn active_tab(state: &EditorState) -> &EditorTab {
    state
        .tabs
        .iter()
        .find(|tab| tab.is_active)
        .expect("应有活动标签")
}

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
    assert!(active_tab(&state).dirty);

    // 非文本按键仍走 keymap → 命令。
    assert!(
        app.dispatch_key("left".to_string(), KeySurface::Editor)
            .unwrap()
            .consumed
    );
    assert!(
        app.dispatch_key("backspace".to_string(), KeySurface::Editor)
            .unwrap()
            .consumed
    );

    let state = app.editor_state();
    assert_eq!(state.text, "i");
    assert_eq!(state.cursor_byte, 0);

    let outcome = app
        .dispatch_key("mod-z".to_string(), KeySurface::Editor)
        .unwrap();
    assert!(outcome.consumed);

    // 没绑定的字符必须返回未消费，让 IME 路径接管。
    assert!(
        !app.dispatch_key("a".to_string(), KeySurface::Editor)
            .unwrap()
            .consumed
    );

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

    assert!(
        app.dispatch_key("tab".to_string(), KeySurface::Editor)
            .unwrap()
            .consumed
    );
    assert!(
        app.dispatch_key("enter".to_string(), KeySurface::Editor)
            .unwrap()
            .consumed
    );
    assert!(
        app.dispatch_key("return".to_string(), KeySurface::Editor)
            .unwrap()
            .consumed
    );

    let state = app.editor_state();
    assert_eq!(state.text, "    \n\n");
    assert_eq!(state.cursor_byte, 6);
    assert!(active_tab(&state).dirty);
}

#[test]
fn panel_toggle_command_should_emit_host_effect() {
    let mut app = App::new();

    // 命中 mod-shift-e → editor 区按下时应被 keymap 消费。
    let outcome = app
        .dispatch_key("mod-shift-e".to_string(), KeySurface::Editor)
        .expect("派发成功");
    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![HostEffect::TogglePanel("file_tree".to_string())]
    );
}

#[test]
fn panel_key_surface_should_keep_global_shortcuts_without_text_edit_context() {
    let mut app = App::new();

    let outcome = app
        .dispatch_key("mod-shift-e".to_string(), KeySurface::Panel)
        .expect("派发成功");
    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![HostEffect::TogglePanel("file_tree".to_string())]
    );

    let outcome = app
        .dispatch_key("mod-a".to_string(), KeySurface::Panel)
        .expect("派发成功");
    assert!(!outcome.consumed);
    assert!(outcome.effects.is_empty());
}

#[test]
fn shortcut_for_should_return_formatted_keymap_binding() {
    let app = App::new();

    // 已绑定的命令：返回格式化后的快捷键。
    let undo = app.shortcut_for(editor::UNDO).expect("undo 必有快捷键");
    let save = app.shortcut_for(editor::SAVE).expect("save 必有快捷键");
    let file_tree = app
        .shortcut_for(PanelId::FileTree.toggle_command_id())
        .expect("file_tree 切换必有快捷键");

    // 平台差异化校验在专门的格式化测试里做；这里只关心"能查到、非空"。
    assert!(!undo.is_empty());
    assert!(!save.is_empty());
    assert!(!file_tree.is_empty());

    let settings = app
        .shortcut_for(settings::OPEN)
        .expect("settings.open 必有快捷键");
    assert!(!settings.is_empty());

    // 未注册的命令：返回 None。
    assert!(app.shortcut_for("不存在的命令").is_none());
}

#[test]
fn command_title_for_should_read_registered_command_metadata() {
    let app = App::new();

    assert_eq!(
        app.command_title_for(project_picker_commands::SHOW_PROJECTS_PICKER)
            .as_deref(),
        Some("切换项目")
    );
    assert_eq!(
        app.command_title_for(PanelId::FileTree.toggle_command_id())
            .as_deref(),
        Some("文件树")
    );

    assert_eq!(
        app.command_title_for(settings::OPEN).as_deref(),
        Some("设置")
    );
    assert_eq!(
        app.command_title_for(diagnostics::SHOW_PROBLEMS).as_deref(),
        Some("诊断")
    );
}

#[test]
fn project_picker_command_should_emit_open_surface_window_action() {
    let mut app = App::new();

    let outcome = app
        .dispatch_key("mod-o".to_string(), KeySurface::Editor)
        .unwrap();

    assert!(outcome.consumed);
    assert_eq!(outcome.effects, vec![HostEffect::ShowProjectPicker]);
}

#[test]
fn open_local_project_command_should_emit_window_action() {
    let mut app = App::new();

    let actions = app
        .dispatch(project_picker_commands::open_local_project())
        .unwrap();

    assert_eq!(actions, vec![HostEffect::OpenLocalProject]);
}

#[test]
fn project_action_commands_should_have_shortcuts_and_emit_effects() {
    let mut app = App::new();

    assert!(
        app.shortcut_for(project_picker_commands::OPEN_LOCAL_PROJECT)
            .is_some()
    );
    assert!(
        app.shortcut_for(project_picker_commands::START_GIT_CLONE)
            .is_some()
    );
    assert!(
        app.shortcut_for(project_picker_commands::REMOVE_RECENT_PROJECT)
            .is_some()
    );

    let outcome = app
        .dispatch_key("down".to_string(), KeySurface::ProjectPicker)
        .unwrap();
    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![HostEffect::ProjectPickerMoveSelection(1)]
    );

    let outcome = app
        .dispatch_key("backspace".to_string(), KeySurface::ProjectPicker)
        .unwrap();
    assert!(!outcome.consumed);
    assert!(outcome.effects.is_empty());

    let outcome = app
        .dispatch_key("enter".to_string(), KeySurface::ProjectPicker)
        .unwrap();
    assert!(outcome.consumed);
    assert_eq!(outcome.effects, vec![HostEffect::ProjectPickerActivate]);

    let actions = app
        .dispatch(project_picker_commands::start_git_clone())
        .unwrap();
    assert_eq!(actions, vec![HostEffect::StartGitClone]);

    let actions = app
        .dispatch(project_picker_commands::remove_recent_project())
        .unwrap();
    assert_eq!(actions, vec![HostEffect::RemoveSelectedRecentProject]);
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
    // 重开项目后工作区清空：没有默认 buffer / 视图，也就没有任何标签。
    let state = app.editor_state();
    assert!(state.tabs.is_empty());
    assert!(state.text.is_empty());
}

#[test]
fn opening_projects_should_maintain_recent_project_records() {
    let mut app = App::new();
    let local = project_fixture("recent-local");
    let cloned = project_fixture("recent-git");

    app.open_local_project(local.clone());
    app.open_git_project(
        cloned.clone(),
        "https://example.com/org/recent-git.git".to_string(),
    );

    let recent = app.recent_projects();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].path, cloned);
    assert_eq!(
        recent[0].identifier,
        "https://example.com/org/recent-git.git"
    );
    assert_eq!(recent[1].path, local);

    let id = recent[0].id.clone();
    app.remove_recent_project(&id);
    let recent = app.recent_projects();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].path, local);
}

#[test]
fn recent_projects_should_persist_to_file() {
    let store = std::env::temp_dir().join(format!(
        "zom-recent-projects-{}-{}.toml",
        std::process::id(),
        "persist"
    ));
    let _ = std::fs::remove_file(&store);
    let local = project_fixture("persist-local");
    let cloned = project_fixture("persist-git");

    {
        let mut app = App::new_with_recent_projects_path(Some(store.clone()));
        app.open_local_project(local.clone());
        app.open_git_project(
            cloned.clone(),
            "https://example.com/org/persist-git.git".to_string(),
        );
    }

    let app = App::new_with_recent_projects_path(Some(store.clone()));
    let recent = app.recent_projects();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].path, cloned);
    assert_eq!(
        recent[0].repo.as_deref(),
        Some("https://example.com/org/persist-git.git")
    );
    assert_eq!(recent[1].path, local);

    let _ = std::fs::remove_file(store);
}

#[test]
fn language_server_status_command_should_emit_open_surface_window_action() {
    let mut app = App::new();

    let actions = app.dispatch(language_servers::open_status()).unwrap();

    assert_eq!(actions, vec![HostEffect::ShowLanguageServers]);
}

#[test]
fn settings_and_diagnostics_commands_should_be_registered() {
    let mut app = App::new();

    let actions = app.dispatch(settings::open()).unwrap();
    assert_eq!(actions, vec![HostEffect::ShowSettings]);

    let actions = app.dispatch(diagnostics::show_problems()).unwrap();
    assert_eq!(actions, vec![HostEffect::ShowDiagnostics]);
}

#[test]
fn escape_should_dispatch_surface_dismiss_command() {
    let mut app = App::new();

    let outcome = app
        .dispatch_key("escape".to_string(), KeySurface::Editor)
        .unwrap();

    assert!(outcome.consumed);
    assert_eq!(outcome.effects, vec![HostEffect::DismissSurface]);
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

#[test]
fn file_tree_pending_editor_keys_route_through_keymap_by_context() {
    let mut app = App::new();
    app.open_local_project(project_fixture("pending-editor"));
    app.file_tree_begin_new_entry(EntryKind::File);

    app.ime_replace_text(None, "alpha").unwrap();

    // 全局快捷键不被单行新建输入框吞掉：在 Global 上下文照常解析成 panel 命令。
    let outcome = app
        .dispatch_key("mod-shift-e".to_string(), KeySurface::FileTree)
        .unwrap();
    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![HostEffect::TogglePanel("file_tree".to_string())]
    );

    // 编辑键在 text_edit 上下文命中，作用到新建输入框（focused_field 路由）。
    let outcome = app
        .dispatch_key("mod-a".to_string(), KeySurface::FileTree)
        .unwrap();
    assert!(outcome.consumed);
    assert!(outcome.effects.is_empty());

    app.ime_replace_text(None, "beta").unwrap();
    let pending = app.file_tree_state().pending.expect("新建输入框仍在编辑态");
    assert_eq!(pending.editor.text, "beta");

    // Enter：单行编辑器不接受换行 → text_edit 落空 → 命中 FileTree 的提交命令。
    let outcome = app
        .dispatch_key("enter".to_string(), KeySurface::FileTree)
        .unwrap();
    assert!(outcome.consumed);
    assert_eq!(outcome.effects, vec![HostEffect::FileTreeCommitNewEntry]);
}

#[test]
fn file_tree_pending_editor_does_not_intercept_keys_while_composing() {
    let mut app = App::new();
    app.open_local_project(project_fixture("pending-ime"));
    app.file_tree_begin_new_entry(EntryKind::File);

    app.ime_replace_and_mark_text(None, "ni", Some(2..2))
        .unwrap();
    assert!(app.ime_marked_range_utf16().is_some());

    // 组合态下 dispatch_key 一律不消费、不拦截：Enter / Esc 等都透传给系统
    // 输入法，由它驱动候选的提交 / 取消。宿主在这里抢键会让 IME 会话脱节。
    let outcome = app
        .dispatch_key("enter".to_string(), KeySurface::FileTree)
        .unwrap();
    assert!(!outcome.consumed);
    assert!(outcome.effects.is_empty());

    // 这次 Enter 没动到任何状态：组合还在，文件树新建也还在。
    assert!(app.ime_marked_range_utf16().is_some());
    assert!(app.file_tree_state().pending.is_some());
}

#[test]
fn file_tree_pending_editor_escape_exits_right_after_ime_preedit_cleared() {
    let mut app = App::new();
    app.open_local_project(project_fixture("pending-ime-esc"));
    app.file_tree_begin_new_entry(EntryKind::File);

    // 输入中文候选。
    app.ime_replace_and_mark_text(None, "ni", Some(2..2))
        .unwrap();
    assert!(app.ime_marked_range_utf16().is_some());

    // 系统输入法取消候选 = 把 marked text 置空。composition 必须彻底结束，
    // 不留空壳 —— 否则 marked_text_range 仍报 Some，系统 IME 会吞掉后续按键。
    app.ime_replace_and_mark_text(None, "", None).unwrap();
    assert!(
        app.ime_marked_range_utf16().is_none(),
        "preedit 清空后 composition 必须彻底结束，不能留空壳"
    );

    // 紧接着一次 Esc 就该真正退出新建。
    let outcome = app
        .dispatch_key("escape".to_string(), KeySurface::FileTree)
        .unwrap();
    assert!(outcome.consumed);
    assert_eq!(outcome.effects, vec![HostEffect::FileTreeCancelNewEntry]);
}

#[test]
fn tab_commands_should_switch_and_close_active_view() {
    let mut app = App::new();
    app.open_local_project(project_fixture("tabs"));

    // 打开 README.md：rows = [root, src, README.md]。
    app.file_tree_move_selection(1); // root
    app.file_tree_move_selection(1); // src
    app.file_tree_move_selection(1); // README.md
    assert_eq!(app.file_tree_activate(), FileTreeActivation::OpenedFile);

    // 展开 src 并打开 src/lib.rs：
    // 展开后 rows = [root, src, inner, lib.rs, README.md]。
    app.file_tree_move_selection(-1); // 回到 src
    app.file_tree_expand_or_into(); // 展开 src
    app.file_tree_move_selection(1); // inner
    app.file_tree_move_selection(1); // lib.rs
    assert_eq!(app.file_tree_activate(), FileTreeActivation::OpenedFile);

    // 两个标签：README.md 先开、lib.rs 后开且为活动标签。
    let state = app.editor_state();
    assert_eq!(state.tabs.len(), 2);
    assert_eq!(active_tab(&state).title, "lib.rs");
    assert!(state.tabs[1].is_active);

    // 切到上一个标签 → README.md。
    app.dispatch(editor::select_tab(editor::SelectTabTarget::Previous))
        .unwrap();
    let state = app.editor_state();
    assert_eq!(active_tab(&state).title, "README.md");
    assert!(state.tabs[0].is_active);

    // 关闭当前标签 → 只剩 lib.rs。
    app.dispatch(editor::close_tab()).unwrap();
    let state = app.editor_state();
    assert_eq!(state.tabs.len(), 1);
    assert_eq!(active_tab(&state).title, "lib.rs");
}
