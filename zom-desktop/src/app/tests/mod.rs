//! `App` 派发管线的 headless 单元测试。
//!
//! 这一层不接触 GPUI ——只覆盖 keymap 解析、命令派发、IME 流，以及命令产出的 HostEffect。
//! 需要 GPUI 句柄（Entity / Window / 焦点等）的链路在 shell 根视图那一层做手工 / 集成测试，不进本文件。

use crate::app::App;
use crate::config::SettingsChange;
use crate::editor_state::{EditorState, EditorTab};
use crate::focus::{AppFocus, FileTreeFocus, PanelFocus, VersionControlFocus};
use crate::host_intent::{InteractionIntent, PointerIntent};
use crate::text_target::{TextTargetOwner, TextTargetQuery};
use crate::theme::Theme;
use crate::ui_id::PanelId;
use std::cell::RefCell;
use std::fs::{File, create_dir_all};
use std::path::PathBuf;
use std::rc::Rc;
use zom_command::commands::{
    diagnostics, editor, file_tree, language_servers, project_picker as project_picker_commands,
    settings,
};
use zom_command::{
    BubbleEffect, EditorEffect, FileTreeEffect, HostEffect, PanelEffect, ProjectEffect,
    SearchEffect, SurfaceEffect,
};
use zom_command::{EditTarget, KeyContext};
use zom_engine::{ByteOffset, SelectionSet};
use zom_workspace::view::{ViewportState, WrapMap};

/// 取当前活动标签——断言「编辑区正在显示哪个文件」用。
fn active_tab(state: &EditorState) -> &EditorTab {
    state
        .tabs
        .iter()
        .find(|tab| tab.is_active())
        .expect("应有活动标签")
}

fn editor_state(app: &App) -> EditorState {
    app.editor_state()
}

fn temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("zom-app-{tag}-{}.toml", std::process::id()))
}

/// 构造一个已打开项目并激活了一个空文件的 `App`。
fn app_with_open_file(name: &str) -> App {
    let mut app = App::new();
    let root = project_fixture(name);
    app.apply_open_project_from_effect(root.clone(), None);
    assert!(app.session.open_file(root.join("README.md")));
    app.request_focus(AppFocus::editor());
    app
}

fn app_with_markdown_text(name: &str, text: &str) -> App {
    let mut app = App::new();
    let root = project_fixture(name);
    std::fs::write(root.join("README.md"), text).unwrap();
    app.apply_open_project_from_effect(root.clone(), None);
    assert!(app.session.open_file(root.join("README.md")));
    app.request_focus(AppFocus::editor());
    app.session
        .workspace()
        .syntax_worker()
        .wait_for_idle_for_test_or_bench();
    app
}

fn active_buffer_text(app: &App) -> String {
    let buffer_id = app.active_buffer_id().expect("应有活动 buffer");
    let buffer = app
        .session
        .workspace()
        .buffer(buffer_id)
        .expect("活动 buffer 应存在")
        .buffer();
    buffer
        .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
        .unwrap()
        .into_text()
        .into_owned()
}

struct StubProjectPickerOwner {
    query: crate::editor::text::OwnedEditorTarget,
}

impl TextTargetQuery for StubProjectPickerOwner {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        focus == AppFocus::project_picker()
    }

    fn snapshot(&self, _focus: AppFocus) -> crate::editor::text::EditorSnapshot {
        self.query
            .snapshot(crate::editor::text::EditorSnapshotRequest::single_line())
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        vec![
            KeyContext::project_picker(),
            KeyContext::text_edit(false, false),
            KeyContext::global(),
        ]
    }

    fn ime_query_target(
        &self,
        _focus: AppFocus,
    ) -> Option<crate::editor::text::ImeQueryTarget<'_>> {
        Some(self.query.as_ime_query_target())
    }
}

impl TextTargetOwner for StubProjectPickerOwner {
    fn edit_target(&mut self, _focus: AppFocus) -> Option<EditTarget<'_>> {
        Some(self.query.as_edit_target())
    }
}

/// 在 headless 测试里模拟 project picker 注册过 text target owner。
fn install_project_picker(app: &mut App) -> Rc<RefCell<StubProjectPickerOwner>> {
    let picker = Rc::new(RefCell::new(StubProjectPickerOwner {
        query: crate::editor::text::OwnedEditorTarget::new(),
    }));
    app.install_editor_owner(picker.clone() as Rc<RefCell<dyn TextTargetOwner>>);
    picker
}

#[test]
fn esc_should_collapse_extended_selection_via_dismiss_stack() {
    // 没有 picker / search bar / pending dialog 等更高瞬态在前，esc 应该塌掉编辑区的非空选区。
    // 这条路径由 command_runtime 末尾的 reconcile_text_edit_dismiss 自动 push 一条 editor.clear_selection token；
    // esc 在 text_edit 上下文走 system.dismiss_top(TextEdit) 把它弹出再派发。
    let mut app = app_with_markdown_text("esc-clear-selection", "hello world");

    app.dispatch_command(editor::select_all()).unwrap();
    let extent_before = !app
        .session
        .active_edit_view()
        .unwrap()
        .selection()
        .primary()
        .is_caret();
    assert!(extent_before, "select_all 之后选区必须非空");

    let outcome = app.dispatch_key("escape".to_string()).unwrap();
    assert!(outcome.consumed, "esc 必须被 dismiss_top 消化");

    let is_caret_after = app
        .session
        .active_edit_view()
        .unwrap()
        .selection()
        .primary()
        .is_caret();
    assert!(is_caret_after, "esc 应当把扩展选区塌成 caret");
}

#[test]
fn esc_should_collapse_pointer_selection_on_first_press() {
    let mut app = app_with_markdown_text("esc-pointer-selection", "hello world");

    app.request_focus(AppFocus::panel(PanelId::Terminal));
    // Pointer interaction 必须同步 slot 对应的语义焦点；
    // 否则 selection 虽然会落到 active view，但后续 Esc 仍按旧焦点解析，第一下不会清选区。
    app.dispatch_interaction(InteractionIntent::Pointer(PointerIntent::SetSelection {
        focus: AppFocus::editor(),
        anchor: ByteOffset::new(1),
        head: ByteOffset::new(5),
    }))
    .unwrap();
    assert!(
        !app.session
            .active_edit_view()
            .unwrap()
            .selection()
            .primary()
            .is_caret(),
        "pointer interaction 产生的选区必须立刻进入 dismiss 栈"
    );

    let outcome = app.dispatch_key("escape".to_string()).unwrap();
    assert!(outcome.consumed, "第一下 esc 必须清掉 pointer 选区");
    assert_eq!(
        outcome.effects,
        vec![HostEffect::Editor(EditorEffect::CancelPointerSelection)],
        "第一下 esc 还必须取消宿主侧鼠标拖选会话，避免 stale mousemove 复活选区"
    );
    assert!(
        app.session
            .active_edit_view()
            .unwrap()
            .selection()
            .primary()
            .is_caret(),
        "pointer 选区不应需要第二下 esc"
    );
}

#[test]
fn pointer_scroll_should_move_viewport_through_interaction_pipeline() {
    let text = (0..100)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut app = app_with_markdown_text("pointer-scroll", &text);
    app.session
        .active_edit_view_mut()
        .expect("应有活动视图")
        .set_viewport(ViewportState {
            top_line: 0,
            top_subrow: 0,
            visible_visual_rows: 5,
            visible_logical_lines: 5,
        });

    app.request_focus(AppFocus::panel(PanelId::Terminal));
    app.dispatch_interaction(InteractionIntent::Pointer(PointerIntent::ScrollViewport {
        focus: AppFocus::editor(),
        delta_visual_rows: 3,
    }))
    .unwrap();

    let viewport = app
        .session
        .active_edit_view()
        .expect("应有活动视图")
        .viewport();
    assert_eq!(viewport.top_line, 3);
    assert_eq!(app.focus().current(), AppFocus::editor());
}

#[test]
fn tab_and_enter_should_dispatch_editor_commands() {
    let mut app = app_with_open_file("tab-enter");

    assert!(app.dispatch_key("tab".to_string()).unwrap().consumed);
    assert!(app.dispatch_key("enter".to_string()).unwrap().consumed);
    assert!(app.dispatch_key("return".to_string()).unwrap().consumed);

    let state = editor_state(&app);

    assert!(matches!(
        active_tab(&state),
        EditorTab::Edit(t) if t.dirty
    ));
}

#[test]
fn settings_changes_should_update_runtime_config_and_persist() {
    let path = temp_path("settings-change");
    let _ = std::fs::remove_file(&path);
    let mut app = App::new_with_paths(Some(path.clone()));

    app.apply_settings_change_from_effect(SettingsChange::AdjustUiFont(1));
    app.apply_settings_change_from_effect(SettingsChange::AdjustEditorFont(2));
    app.apply_settings_change_from_effect(SettingsChange::ToggleEditorSoftWrap);

    let config = app.config_snapshot();
    assert_eq!(config.general.theme, Theme::System.as_config());
    assert_eq!(config.ui.font_size, 14);
    assert_eq!(config.editor.font_size, 18);
    assert!(!config.editor.soft_wrap);

    let (loaded, warnings) = crate::config::AppConfig::load(Some(&path));
    assert_eq!(loaded, config);
    assert!(warnings.is_empty());

    let _ = std::fs::remove_file(path);
}

#[test]
fn open_config_file_should_create_and_focus_main_editor_tab() {
    let path = temp_path("open-config-file");
    let _ = std::fs::remove_file(&path);
    let mut app = App::new_with_paths(Some(path.clone()));

    assert!(app.apply_open_config_file_from_effect());
    assert!(path.exists());
    assert_eq!(app.focus().current(), AppFocus::editor());

    let state = editor_state(&app);
    let active = active_tab(&state);
    assert_eq!(
        active.title(),
        path.file_name()
            .expect("临时配置路径应有文件名")
            .to_string_lossy()
            .into_owned()
            .as_str()
    );
    assert!(matches!(active, EditorTab::Edit(t) if t.language == "TOML"));

    let _ = std::fs::remove_file(path);
}

#[test]
fn editor_tab_size_setting_should_reach_open_buffers() {
    let mut app = App::new();
    // 从默认 4 切到 2。
    app.apply_settings_change_from_effect(SettingsChange::CycleEditorTabSize);
    let root = project_fixture("tab-size-setting");
    app.apply_open_project_from_effect(root.clone(), None);
    assert!(app.session.open_file(root.join("README.md")));

    let tab_width = app
        .active_buffer_id()
        .and_then(|id| app.workspace().buffer(id))
        .expect("应有活动 buffer")
        .buffer()
        .config()
        .tab
        .tab_width();
    assert_eq!(tab_width, 2);
}

#[test]
fn text_edit_should_preserve_conservative_active_view_wrap_map() {
    let mut app = app_with_markdown_text("preserve-wrap-map", "abcdefghij\nklmnopqrst");
    app.session
        .active_edit_view_mut()
        .expect("应有活动视图")
        .set_wrap_map(Some(WrapMap::new(true, vec![vec![5], vec![4]])));

    app.dispatch_command(editor::insert_text("X")).unwrap();

    let wrap_map = app
        .session
        .active_edit_view()
        .expect("应有活动视图")
        .wrap_map()
        .expect("文本变化后应保留一份保守 wrap map");
    assert_eq!(wrap_map.logical_line_count(), 2);
    assert!(wrap_map.breaks(0).is_empty());
    assert_eq!(wrap_map.breaks(1), &[4]);
}

#[test]
fn desktop_text_input_should_merge_consecutive_same_edit_commands_for_undo_redo() {
    let mut app = app_with_markdown_text("merge-text-input-history", "");

    app.dispatch_command(editor::insert_text("a")).unwrap();
    app.dispatch_command(editor::insert_text("b")).unwrap();
    app.dispatch_command(editor::insert_text("c")).unwrap();

    assert_eq!(active_buffer_text(&app), "abc");

    app.dispatch_command(editor::undo()).unwrap();
    assert_eq!(active_buffer_text(&app), "");

    app.dispatch_command(editor::redo()).unwrap();
    assert_eq!(active_buffer_text(&app), "abc");
}

#[test]
fn text_edit_without_soft_wrap_should_refresh_wrap_map_line_count_immediately() {
    let mut app = app_with_open_file("refresh-nowrap-map");
    app.apply_settings_change_from_effect(SettingsChange::ToggleEditorSoftWrap);
    assert!(!app.config_snapshot().editor.soft_wrap);
    app.session
        .active_edit_view_mut()
        .expect("应有活动视图")
        .set_wrap_map(Some(WrapMap::sparse(false, 1, [])));

    app.dispatch_command(editor::insert_newline()).unwrap();

    let wrap_map = app
        .session
        .active_edit_view()
        .expect("应有活动视图")
        .wrap_map()
        .expect("关闭软换行时应立即刷新逻辑行视觉模型");
    assert!(!wrap_map.soft_wrap());
    assert_eq!(wrap_map.logical_line_count(), 2);
    assert_eq!(wrap_map.total_visual_rows(), 2);
}

/// 收集一份 snapshot 内属于 syntax 的 Foreground decoration（`(start, end, name)`）。
/// 单独抽出 syntax 段方便对比 edit-frame 与 reparse-frame。
fn syntax_decorations(
    snapshot: &crate::editor::text::EditorSnapshot,
) -> Vec<(usize, usize, String)> {
    let mut out: Vec<_> = snapshot
        .decorations
        .iter()
        .filter(|d| {
            d.kind == crate::editor::highlight::DecorationKind::Foreground
                && d.priority == crate::editor::highlight::priority::SYNTAX
        })
        .filter_map(|d| match &d.style {
            crate::editor::highlight::DecorationStyle(
                crate::editor::highlight::StyleClass::Syntax(name),
            ) => Some((d.range.start().get(), d.range.end().get(), name.clone())),
            _ => None,
        })
        .collect();
    out.sort();
    out
}

/// 不变量：在 token 内部插入字符（结构未变）后立即取 snapshot，syntax
/// decoration 必须覆盖新插入字节 —— 主线程 `tree.edit` 把 slot 推进到新版本，
/// paint 端按 viewport 现查 Query 就能命中 shifted node。
///
/// 这条钉死「token 内插入不会闪默认前景色」—— 主线程 `tree_slot.try_edit` 的存在理由。
#[test]
fn edit_immediately_extends_syntax_decoration_inside_heading_token() {
    // 在 `# zom 文档规范` 的 `zom` 中间插入字符：byte 4（'z' 与 'o' 之间）。
    // heading_content 节点 [2..18] 跨越插入点，tree.edit 后变为 [2..19]，
    // 第一帧 paint 跑 query 应当命中扩展后的 heading 段，新字节 [4..5) 在内。
    let mut app = app_with_markdown_text("token-inside-edit", "# zom 文档规范\n\n正文。\n");
    *app.session
        .active_edit_view_mut()
        .expect("应有活动视图")
        .selection_mut() = SelectionSet::caret(ByteOffset::new(4));

    app.dispatch_command(editor::replace_selection("X"))
        .unwrap();

    let snapshot = app.with_router(|router| router.snapshot_for_focus(AppFocus::editor()));
    assert!(
        snapshot.decorations.iter().any(|d| d.kind
            == crate::editor::highlight::DecorationKind::Foreground
            && d.range.start().get() <= 4
            && d.range.end().get() >= 5
            && d.priority == crate::editor::highlight::priority::SYNTAX),
        "dispatch 后立即 snapshot 必须包含覆盖新字节 [4, 5) 的 syntax decoration，实际 {:?}",
        snapshot.decorations,
    );
}

/// 关键不变量：结构未变的小编辑下，edit 后立即 paint 的 syntax decoration
/// 必须**逐项等于** worker reparse 完成后的 paint 结果。
///
/// `tree.edit` 只推坐标不改结构 —— interpolate tree 跑出的 query 与重 parse
/// 后的 query 在 viewport 上应当命中同一组 node。一帧不闪、不糊。
#[test]
fn edit_frame_decorations_equal_reparse_frame_for_structure_preserving_edit() {
    let mut app = app_with_markdown_text("no-flash", "# zom 文档规范\n\n正文段落。\n");
    *app.session
        .active_edit_view_mut()
        .expect("应有活动视图")
        .selection_mut() = SelectionSet::caret(ByteOffset::new(4));

    app.dispatch_command(editor::replace_selection("X"))
        .unwrap();

    // edit-frame：worker 还没回包，slot 里只有主线程 tree.edit 推进的 interpolate tree。
    let edit_frame = app.with_router(|router| router.snapshot_for_focus(AppFocus::editor()));
    let edit_frame_syntax = syntax_decorations(&edit_frame);

    // reparse-frame：等 worker 把真正的重 parse 结果 store 回 slot。
    app.session
        .workspace()
        .syntax_worker()
        .wait_for_idle_for_test_or_bench();
    let reparse_frame = app.with_router(|router| router.snapshot_for_focus(AppFocus::editor()));
    let reparse_frame_syntax = syntax_decorations(&reparse_frame);

    assert_eq!(
        edit_frame_syntax, reparse_frame_syntax,
        "结构未变的小编辑下 edit-frame 与 reparse-frame 必须产出相同 syntax decoration —— \
         否则就会出现一帧错色 flash。\n  edit-frame: {edit_frame_syntax:?}\n  reparse-frame: {reparse_frame_syntax:?}",
    );
}

#[test]
fn panel_toggle_command_should_emit_host_effect() {
    let mut app = App::new();

    // 命中 mod shift e → editor 区按下时应被 keymap 消费。
    let outcome = app
        .dispatch_key("mod shift e".to_string())
        .expect("派发成功");
    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![
            HostEffect::Panel(PanelEffect::Toggle(PanelId::FileTree, false)),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );
}

#[test]
fn search_shortcut_should_emit_activate_effect() {
    let mut app = App::new();
    // 搜索快捷键限定在 text_edit 上下文内；空 focus 不响应。
    app.request_focus(AppFocus::editor());

    let outcome = app.dispatch_key("mod f".to_string()).expect("派发成功");
    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![
            HostEffect::Search(SearchEffect::Toggle),
            HostEffect::Editor(EditorEffect::CancelPointerSelection)
        ]
    );

    // mod shift f 绑到项目搜索占位命令：弹一条"敬请期待"气泡。
    let outcome = app
        .dispatch_key("mod shift f".to_string())
        .expect("派发成功");
    assert!(outcome.consumed);
    assert_eq!(outcome.effects.len(), 2);
    assert!(matches!(
        outcome.effects[0],
        HostEffect::Bubble(BubbleEffect::Show(_))
    ));
    assert!(matches!(
        outcome.effects[1],
        HostEffect::Editor(EditorEffect::CancelPointerSelection)
    ));
}

#[test]
fn panel_key_surface_should_keep_global_shortcuts_without_text_edit_context() {
    let mut app = App::new();
    app.request_focus(AppFocus::panel(PanelId::Terminal));

    let outcome = app
        .dispatch_key("mod shift e".to_string())
        .expect("派发成功");
    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![
            HostEffect::Panel(PanelEffect::Toggle(PanelId::FileTree, false)),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );

    let outcome = app.dispatch_key("mod a".to_string()).expect("派发成功");
    assert!(!outcome.consumed);
    assert!(outcome.effects.is_empty());
}

#[test]
fn shortcuts_for_should_return_formatted_keymap_binding() {
    let app = App::new();

    // 已绑定的命令：返回格式化后的快捷键。
    let undo = app.shortcuts_for(editor::UNDO).expect("undo 必有快捷键");
    let save = app.shortcuts_for(editor::SAVE).expect("save 必有快捷键");
    let file_tree = app
        .shortcuts_for(PanelId::FileTree.toggle_command_id())
        .expect("file_tree 切换必有快捷键");

    // 平台差异化校验在专门的格式化测试里做；这里只关心"能查到、非空"。
    assert!(!undo.is_empty());
    assert!(!save.is_empty());
    assert!(!file_tree.is_empty());

    let settings = app
        .shortcuts_for(settings::OPEN)
        .expect("settings.open 必有快捷键");
    assert!(!settings.is_empty());

    // 未注册的命令：返回 None。
    assert!(app.shortcuts_for("不存在的命令").is_none());
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

    let outcome = app.dispatch_key("mod o".to_string()).unwrap();

    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![
            HostEffect::Project(ProjectEffect::ShowPicker),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );
}

#[test]
fn open_local_project_command_should_emit_window_action() {
    let mut app = App::new();
    app.request_focus(AppFocus::project_picker());

    let actions = app
        .dispatch_command(project_picker_commands::open_local_project())
        .unwrap();

    assert_eq!(
        actions,
        vec![
            HostEffect::Project(ProjectEffect::OpenLocalProject),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );
}

#[test]
fn project_action_commands_should_have_shortcuts_and_emit_effects() {
    let mut app = App::new();
    let _picker = install_project_picker(&mut app);

    assert!(
        app.shortcuts_for(project_picker_commands::OPEN_LOCAL_PROJECT)
            .is_some()
    );
    assert!(
        app.shortcuts_for(project_picker_commands::START_GIT_CLONE)
            .is_some()
    );
    assert!(
        app.shortcuts_for(project_picker_commands::REMOVE_RECENT_PROJECT)
            .is_some()
    );

    let outcome = app.dispatch_key("down".to_string()).unwrap();
    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![
            HostEffect::Project(ProjectEffect::MovePickerSelection(1)),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );

    // backspace 落到 picker query 的 text_edit 上下文，由 DELETE 命令处理（删一个字符）。
    // 不是 picker 的导航动作，但仍由 keymap 消费。
    let outcome = app.dispatch_key("backspace".to_string()).unwrap();
    assert!(outcome.consumed);

    let outcome = app.dispatch_key("enter".to_string()).unwrap();
    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![
            HostEffect::Project(ProjectEffect::ActivatePicker),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );

    let actions = app
        .dispatch_command(project_picker_commands::start_git_clone())
        .unwrap();
    assert_eq!(
        actions,
        vec![
            HostEffect::Project(ProjectEffect::StartGitClone),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );

    let actions = app
        .dispatch_command(project_picker_commands::remove_recent_project())
        .unwrap();
    assert_eq!(
        actions,
        vec![
            HostEffect::Project(ProjectEffect::RemoveSelectedRecentProject),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );
}

#[test]
fn file_tree_confirm_delete_focus_should_route_enter_and_escape_to_dialog_actions() {
    let mut app = App::new();
    // Esc 改走 system.dismiss_top 后，必须经 request_delete() 才能把 cancel token 推上栈。
    let _ = app.dispatch_command(file_tree::request_delete()).unwrap();
    app.request_focus(AppFocus::file_tree(FileTreeFocus::ConfirmDelete));
    app.request_focus_from_shell(AppFocus::file_tree(FileTreeFocus::Navigate));
    assert_eq!(
        app.focus().current(),
        AppFocus::file_tree(FileTreeFocus::ConfirmDelete)
    );

    let outcome = app.dispatch_key("enter".to_string()).unwrap();
    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![
            HostEffect::FileTree(FileTreeEffect::ConfirmDelete),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );

    // enter 提交后栈已被 commit 清空；再 esc 必须重新经 request_delete() 才有 token。
    let _ = app.dispatch_command(file_tree::request_delete()).unwrap();
    app.request_focus(AppFocus::file_tree(FileTreeFocus::Navigate));
    assert_eq!(
        app.focus().current(),
        AppFocus::file_tree(FileTreeFocus::Navigate)
    );
    app.request_focus(AppFocus::file_tree(FileTreeFocus::ConfirmDelete));

    let outcome = app.dispatch_key("escape".to_string()).unwrap();
    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![
            HostEffect::FileTree(FileTreeEffect::CancelDelete),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );
}

#[test]
fn project_title_should_prompt_when_no_project_is_open() {
    let app = App::new();

    assert_eq!(app.project_title(), "打开项目");
}

// RecentProjects 的 remember / remove / 落盘语义现在归 picker runtime 拥有，
// 单测落在 project picker recent 模块，App 不再覆盖。

#[test]
fn language_server_status_command_should_emit_open_surface_window_action() {
    let mut app = App::new();

    let actions = app.dispatch_command(language_servers::open()).unwrap();

    assert_eq!(
        actions,
        vec![
            HostEffect::Surface(SurfaceEffect::ShowLanguageServers),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );
}

#[test]
fn settings_and_diagnostics_commands_should_be_registered() {
    let mut app = App::new();

    let actions = app.dispatch_command(settings::open()).unwrap();
    assert_eq!(
        actions,
        vec![
            HostEffect::Surface(SurfaceEffect::ShowSettings),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );

    let actions = app.dispatch_command(settings::dismiss()).unwrap();
    assert_eq!(
        actions,
        vec![
            HostEffect::Surface(SurfaceEffect::Dismiss),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );

    let actions = app.dispatch_command(diagnostics::show_problems()).unwrap();
    assert_eq!(
        actions,
        vec![
            HostEffect::Surface(SurfaceEffect::ShowDiagnostics),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );
}

#[test]
fn settings_escape_should_dispatch_settings_dismiss_command() {
    // Esc 现在走 system.dismiss_top —— 必须先经 settings::open()
    // push 一条 dismiss token，否则栈空 esc 静默。
    let mut app = App::new();
    let _ = app.dispatch_command(settings::open()).unwrap();
    app.request_focus(AppFocus::settings());

    let outcome = app.dispatch_key("escape".to_string()).unwrap();

    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![
            HostEffect::Surface(SurfaceEffect::Dismiss),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );
}

#[test]
fn language_servers_escape_should_dispatch_dismiss_command() {
    let mut app = App::new();
    let _ = app.dispatch_command(language_servers::open()).unwrap();
    app.request_focus(AppFocus::language_servers());

    let outcome = app.dispatch_key("escape".to_string()).unwrap();

    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![
            HostEffect::Surface(SurfaceEffect::Dismiss),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );
}

#[test]
fn project_picker_escape_should_dispatch_project_picker_dismiss_command() {
    // Esc 不再静态绑到 DISMISS；现在它走 DISMISS_TOP，先弹 DismissScope::ProjectPicker 的栈顶。
    // 因此真正的取消能力依赖于 SHOW_PROJECTS_PICKER 在打开 picker 时 push 一条
    // dismiss token；没 push（host 走非命令路径直接打开 picker）esc 就静默。
    let mut app = App::new();
    let _picker = install_project_picker(&mut app);
    // 走命令路径打开 picker：SHOW_PROJECTS_PICKER push token + emit ShowProjectPicker。
    let _ = app
        .dispatch_command(project_picker_commands::show_projects_picker())
        .unwrap();
    app.request_focus(AppFocus::project_picker());

    let outcome = app.dispatch_key("escape".to_string()).unwrap();

    assert!(outcome.consumed);
    assert_eq!(
        outcome.effects,
        vec![
            HostEffect::Surface(SurfaceEffect::Dismiss),
            HostEffect::Editor(EditorEffect::CancelPointerSelection),
        ]
    );
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
fn tab_commands_should_switch_and_close_active_view() {
    let mut app = App::new();
    let root = project_fixture("tabs");
    app.apply_open_project_from_effect(root.clone(), None);
    assert!(app.session.open_file(root.join("README.md")));
    assert!(app.session.open_file(root.join("src/lib.rs")));

    // 两个标签：README.md 先开、lib.rs 后开且为活动标签。
    let state = editor_state(&app);
    assert_eq!(state.tabs.len(), 2);
    assert_eq!(active_tab(&state).title(), "lib.rs");
    assert!(state.tabs[1].is_active());

    // 切到上一个标签 → README.md。
    app.dispatch_command(editor::select_tab(editor::SelectTabTarget::Previous))
        .unwrap();
    let state = editor_state(&app);
    assert_eq!(active_tab(&state).title(), "README.md");
    assert!(state.tabs[0].is_active());

    // 关闭当前标签 → 只剩 lib.rs。
    app.dispatch_command(editor::close_active_tab()).unwrap();
    let state = editor_state(&app);
    assert_eq!(state.tabs.len(), 1);
    assert_eq!(active_tab(&state).title(), "lib.rs");
}

/// EditorTargetRegistry 集成契约：runtime 注册进来的 owner 能被 router
/// 通过 `accepts_focus` 找到并落到 query / 命令写入路径上。
///
/// 该测试是 §2 拆分 App 字段的基础——后续每个 model 迁出时只需把自己
/// 注册到 registry，不再在 App struct 上长字段。本用例守住该机制本身。
mod registry_integration {
    use super::*;
    use crate::editor::text::{
        EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, OwnedEditorTarget,
    };
    use crate::focus::FileTreeFocus;
    use crate::text_target::{TextTargetOwner, TextTargetQuery};
    use std::cell::RefCell;
    use std::rc::Rc;
    use zom_command::{EditTarget, FileTreeKeyMode, KeyContext, VersionControlKeyMode};

    /// 自定义 focus 的桩 owner：accepts_focus 只命中一个普通 panel；
    /// after_text_changed 翻一个 flag 让 router 写路径可观察。
    struct StubPanelOwner {
        flag: std::cell::Cell<bool>,
    }

    impl StubPanelOwner {
        fn new() -> Self {
            Self {
                flag: std::cell::Cell::new(false),
            }
        }
    }

    impl TextTargetQuery for StubPanelOwner {
        fn accepts_focus(&self, focus: AppFocus) -> bool {
            focus == AppFocus::panel(PanelId::KeyboardShortcuts)
        }
        fn snapshot(&self, _focus: AppFocus) -> EditorSnapshot {
            EditorSnapshot::default()
        }
        fn key_contexts(&self) -> Vec<KeyContext> {
            vec![KeyContext::settings(), KeyContext::global()]
        }
        fn ime_query_target(&self, _focus: AppFocus) -> Option<ImeQueryTarget<'_>> {
            None
        }
    }

    impl TextTargetOwner for StubPanelOwner {
        fn edit_target(&mut self, _focus: AppFocus) -> Option<EditTarget<'_>> {
            None
        }
        fn after_text_changed(&mut self) {
            self.flag.set(true);
        }
    }

    struct StubFileTreeNameOwner {
        active: std::cell::Cell<bool>,
        target: OwnedEditorTarget,
    }

    impl StubFileTreeNameOwner {
        fn new() -> Self {
            Self {
                active: std::cell::Cell::new(false),
                target: OwnedEditorTarget::new(),
            }
        }

        fn set_active(&self, active: bool) {
            self.active.set(active);
        }

        fn text(&self) -> String {
            self.target.text()
        }
    }

    impl TextTargetQuery for StubFileTreeNameOwner {
        fn accepts_focus(&self, focus: AppFocus) -> bool {
            self.active.get() && focus == AppFocus::file_tree(FileTreeFocus::NewEntryName)
        }

        fn snapshot(&self, _focus: AppFocus) -> EditorSnapshot {
            self.target.snapshot(EditorSnapshotRequest::single_line())
        }

        fn key_contexts(&self) -> Vec<KeyContext> {
            vec![
                KeyContext::text_edit(false, false),
                KeyContext::file_tree(FileTreeKeyMode::PendingName),
                KeyContext::global(),
            ]
        }

        fn ime_query_target(&self, _focus: AppFocus) -> Option<ImeQueryTarget<'_>> {
            Some(self.target.as_ime_query_target())
        }
    }

    impl TextTargetOwner for StubFileTreeNameOwner {
        fn edit_target(&mut self, _focus: AppFocus) -> Option<EditTarget<'_>> {
            Some(self.target.as_edit_target())
        }
    }

    #[test]
    fn registered_owner_is_reachable_via_router_key_contexts() {
        let mut app = App::new();
        let owner = Rc::new(RefCell::new(StubPanelOwner::new()));
        let dyn_owner: Rc<RefCell<dyn TextTargetOwner>> = owner.clone();
        app.install_editor_owner(dyn_owner);

        let focus = AppFocus::panel(PanelId::KeyboardShortcuts);
        let contexts = app.with_router(|router| router.key_contexts_for(focus));
        let contexts = contexts.expect("focus 应被 stub owner 接管");
        assert!(contexts.iter().any(|c| c == &KeyContext::settings()));
    }

    #[test]
    fn registered_owner_does_not_steal_other_focuses() {
        let mut app = App::new();
        let owner: Rc<RefCell<dyn TextTargetOwner>> = Rc::new(RefCell::new(StubPanelOwner::new()));
        app.install_editor_owner(owner);

        // Editor focus 不在 stub 的 accepts_focus 范围内——应当落到主编辑区 owner，
        // 主编辑区无活动 view 时仍返回它自己的 key_contexts（accepts_newline=true 的 text_edit 栈）。
        let contexts = app.with_router(|router| router.key_contexts_for(AppFocus::editor()));
        assert!(
            contexts.is_some(),
            "Editor focus 应仍由主编辑区 owner 接管，不被 stub 抢走"
        );
    }

    #[test]
    fn file_tree_inline_focus_survives_coarse_shell_projection() {
        let mut app = App::new();
        let owner = Rc::new(RefCell::new(StubFileTreeNameOwner::new()));
        let dyn_owner: Rc<RefCell<dyn TextTargetOwner>> = owner.clone();
        app.install_editor_owner(dyn_owner);

        owner.borrow().set_active(true);
        app.request_focus(AppFocus::file_tree(FileTreeFocus::NewEntryName));

        // 文件树导航、内联新建、内联重命名共用同一个 GPUI FocusHandle。
        // Shell 反向同步只能看出粗粒度 Navigate；App 需要保留仍有效的输入态，
        // 否则 IME commit 会落回主编辑区并在空工作区报 NoActiveView。
        app.request_focus_from_shell(AppFocus::file_tree(FileTreeFocus::Navigate));
        assert_eq!(
            app.focus().current(),
            AppFocus::file_tree(FileTreeFocus::NewEntryName)
        );

        app.dispatch_command(editor::ime_commit(None, "zom"))
            .unwrap();
        assert_eq!(owner.borrow().text(), "zom");
    }

    struct StubVcOwner {
        target: OwnedEditorTarget,
    }

    impl StubVcOwner {
        fn new() -> Self {
            Self {
                target: OwnedEditorTarget::new(),
            }
        }

        fn text(&self) -> String {
            self.target.text()
        }
    }

    impl TextTargetQuery for StubVcOwner {
        fn accepts_focus(&self, focus: AppFocus) -> bool {
            matches!(
                focus,
                AppFocus::Panel(p) if matches!(p.as_version_control(), Some(VersionControlFocus::CommitMessage))
            )
        }

        fn snapshot(&self, _focus: AppFocus) -> EditorSnapshot {
            self.target.snapshot(EditorSnapshotRequest::viewport(0, 5))
        }

        fn key_contexts(&self) -> Vec<KeyContext> {
            vec![
                KeyContext::version_control(VersionControlKeyMode::CommitMessage),
                KeyContext::text_edit(true, false),
                KeyContext::global(),
            ]
        }

        fn ime_query_target(&self, _focus: AppFocus) -> Option<ImeQueryTarget<'_>> {
            Some(self.target.as_ime_query_target())
        }
    }

    impl TextTargetOwner for StubVcOwner {
        fn edit_target(&mut self, _focus: AppFocus) -> Option<EditTarget<'_>> {
            Some(self.target.as_edit_target())
        }
    }

    #[test]
    fn vc_commit_message_dispatch_works_via_registered_owner() {
        let mut app = App::new();
        let owner = Rc::new(RefCell::new(StubVcOwner::new()));
        let dyn_owner: Rc<RefCell<dyn TextTargetOwner>> = owner.clone();
        app.install_editor_owner(dyn_owner);

        app.request_focus(AppFocus::Panel(PanelFocus::version_control_commit()));
        app.dispatch_command(editor::ime_commit(None, "test"))
            .unwrap();
        assert_eq!(owner.borrow().text(), "test");
    }
}
