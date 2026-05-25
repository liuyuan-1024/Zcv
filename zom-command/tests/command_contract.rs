use std::cell::RefCell;
use std::rc::Rc;

use zom_command::commands::{
    debug, diagnostics,
    editor::{self, InsertTextArgs, MoveSelectionArgs, ReplaceSelectionArgs},
    file_tree, keyboard_shortcuts, outline, search, settings, terminal, version_control,
};
use zom_command::{
    Command, CommandArgs, CommandContext, CommandError, CommandExecutor, CommandId, CommandQueue,
    CommandRegistry, EffectQueue, FileTreeKeyMode, HostEffect, KeyBinding, KeyBindingContext,
    KeyChord, KeyContext, Keymap, KeymapResolution, NoArgs, SearchOption, SearchScope,
};
use zom_engine::{ByteOffset, Motion, MovementDirection, MovementUnit, Selection, SelectionSet};
use zom_view::ViewSet;
use zom_workspace::{BufferId, Workspace};

fn command_id(value: &str) -> CommandId {
    CommandId::new(value).unwrap()
}

fn key(value: &str) -> KeyChord {
    KeyChord::new(value).unwrap()
}

fn global_context() -> [KeyContext; 1] {
    [KeyContext::global()]
}

fn text_edit_context() -> [KeyContext; 1] {
    [KeyContext::text_edit(false, false)]
}

fn multiline_text_edit_context() -> [KeyContext; 1] {
    [KeyContext::text_edit(true, false)]
}

fn composing_text_edit_context() -> [KeyContext; 1] {
    [KeyContext::text_edit(false, true)]
}

fn search_panel_context() -> [KeyContext; 1] {
    [KeyContext::search_panel()]
}

fn file_tree_context(mode: FileTreeKeyMode) -> [KeyContext; 1] {
    [KeyContext::file_tree(mode)]
}

fn byte(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn setup(text: &str) -> (Workspace, ViewSet, BufferId) {
    let mut workspace = Workspace::new();
    let buffer_id = workspace.open_text(None, text).unwrap();
    let version = workspace.buffer(buffer_id).unwrap().buffer().version();
    let mut views = ViewSet::new();
    views.open_view(buffer_id, version);
    (workspace, views, buffer_id)
}

fn run(
    registry: &CommandRegistry,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    calls: Vec<(&str, CommandArgs)>,
) -> Result<(), CommandError> {
    let mut queue = CommandQueue::new();
    for (id, args) in calls {
        queue.dispatch(command_id(id), args);
    }

    let mut effects = EffectQueue::new();
    let mut context = CommandContext {
        workspace,
        views,
        focused_field: None,
        queue: &mut queue,
        effects: &mut effects,
    };
    CommandExecutor::new().run(registry, &mut context)
}

fn run_and_collect_effects(
    registry: &CommandRegistry,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    calls: Vec<(&str, CommandArgs)>,
) -> Result<Vec<HostEffect>, CommandError> {
    let mut queue = CommandQueue::new();
    for (id, args) in calls {
        queue.dispatch(command_id(id), args);
    }

    let mut effects = EffectQueue::new();
    let mut context = CommandContext {
        workspace,
        views,
        focused_field: None,
        queue: &mut queue,
        effects: &mut effects,
    };
    CommandExecutor::new().run(registry, &mut context)?;
    Ok(effects.drain())
}

fn text(workspace: &Workspace, buffer_id: BufferId) -> String {
    workspace
        .buffer(buffer_id)
        .unwrap()
        .buffer()
        .text()
        .into_owned()
}

#[test]
fn install_all_should_register_every_builtin_command_catalog() {
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    zom_command::commands::install_all(&mut registry, &mut keymap);

    let registered_titles = [
        (settings::OPEN, "设置"),
        (diagnostics::SHOW_PROBLEMS, "诊断"),
        (file_tree::TOGGLE_PANEL, "文件树"),
        (version_control::TOGGLE_PANEL, "版本管理"),
        (outline::TOGGLE_PANEL, "大纲"),
        (search::TOGGLE_PANEL, "搜索"),
        (search::SCOPE_CURRENT_FILE, "当前文件"),
        (search::SCOPE_PROJECT, "整个项目"),
        (search::TOGGLE_CASE_SENSITIVE, "区分大小写"),
        (search::TOGGLE_WHOLE_WORD, "全词匹配"),
        (search::TOGGLE_REGEX, "正则表达式"),
        (search::FIND_PREVIOUS, "上一个"),
        (search::FIND_NEXT, "下一个"),
        (search::REPLACE_NEXT, "替换下一个"),
        (search::REPLACE_ALL, "全部替换"),
        (terminal::TOGGLE_PANEL, "终端"),
        (debug::TOGGLE_PANEL, "调试"),
        (keyboard_shortcuts::TOGGLE_PANEL, "快捷键"),
    ];

    for (id, title) in registered_titles {
        let id = command_id(id);
        let command = registry.command(&id).expect("命令必须注册");
        assert_eq!(command.title, title);
    }

    assert!(
        keymap
            .format_shortcut_for(&command_id(settings::OPEN))
            .is_some()
    );
    assert!(
        keymap
            .format_shortcut_for(&command_id(file_tree::TOGGLE_PANEL))
            .is_some()
    );
    assert!(
        keymap
            .format_shortcut_for(&command_id(search::TOGGLE_CASE_SENSITIVE))
            .is_some()
    );
    assert!(
        keymap
            .format_shortcut_for(&command_id(search::REPLACE_ALL))
            .is_some()
    );

    let save = registry
        .command(&command_id(editor::SAVE))
        .expect("保存命令必须注册");
    assert_eq!(save.description.as_deref(), Some("保存当前打开的文件。"));
    assert!(save.visible_in_shortcuts);
}

#[test]
fn visible_shortcut_panel_commands_should_have_descriptions_and_shortcuts() {
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    zom_command::commands::install_all(&mut registry, &mut keymap);

    let visible: Vec<_> = registry
        .commands()
        .filter(|command| command.visible_in_shortcuts)
        .collect();

    assert!(
        visible.len() >= 30,
        "应有足够多的内建快捷键命令进入快捷键面板"
    );

    for command in visible {
        assert!(
            command
                .description
                .as_deref()
                .is_some_and(|text| !text.is_empty()),
            "{} 缺少快捷键面板描述",
            command.id
        );
        assert!(
            keymap.format_shortcut_for(&command.id).is_some(),
            "{} 标记进入快捷键面板，但没有快捷键",
            command.id
        );
    }
}

#[test]
fn search_ui_commands_should_emit_state_effects() {
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    search::install(&mut registry, &mut keymap);
    let (mut workspace, mut views, _) = setup("");

    let effects = run_and_collect_effects(
        &registry,
        &mut workspace,
        &mut views,
        vec![
            (search::SCOPE_CURRENT_FILE, CommandArgs::new()),
            (search::SCOPE_PROJECT, CommandArgs::new()),
            (search::TOGGLE_CASE_SENSITIVE, CommandArgs::new()),
            (search::TOGGLE_WHOLE_WORD, CommandArgs::new()),
            (search::TOGGLE_REGEX, CommandArgs::new()),
            (search::FIND_PREVIOUS, CommandArgs::new()),
            (search::FIND_NEXT, CommandArgs::new()),
            (search::REPLACE_NEXT, CommandArgs::new()),
            (search::REPLACE_ALL, CommandArgs::new()),
        ],
    )
    .unwrap();

    assert_eq!(
        effects,
        vec![
            HostEffect::SearchSetScope(SearchScope::CurrentFile),
            HostEffect::SearchSetScope(SearchScope::Project),
            HostEffect::SearchToggleOption(SearchOption::CaseSensitive),
            HostEffect::SearchToggleOption(SearchOption::WholeWord),
            HostEffect::SearchToggleOption(SearchOption::Regex),
            HostEffect::SearchFindPrevious,
            HostEffect::SearchFindNext,
            HostEffect::SearchReplaceNext,
            HostEffect::SearchReplaceAll,
        ]
    );
}

#[test]
fn search_tab_keys_should_resolve_only_in_search_panel_context() {
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    search::install(&mut registry, &mut keymap);
    let search_panel = search_panel_context();
    let text_edit = text_edit_context();
    let global = global_context();

    assert_eq!(
        keymap.resolve(&[key("tab")], &search_panel),
        KeymapResolution::Matched {
            command: command_id(search::FOCUS_NEXT_FIELD),
            args: CommandArgs::new(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("shift-tab")], &search_panel),
        KeymapResolution::Matched {
            command: command_id(search::FOCUS_PREVIOUS_FIELD),
            args: CommandArgs::new(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("tab")], &text_edit),
        KeymapResolution::NoMatch
    );
    assert_eq!(
        keymap.resolve(&[key("tab")], &global),
        KeymapResolution::NoMatch
    );

    let (mut workspace, mut views, _) = setup("");
    let effects = run_and_collect_effects(
        &registry,
        &mut workspace,
        &mut views,
        vec![
            (search::FOCUS_NEXT_FIELD, CommandArgs::new()),
            (search::FOCUS_PREVIOUS_FIELD, CommandArgs::new()),
        ],
    )
    .unwrap();
    assert_eq!(
        effects,
        vec![
            HostEffect::SearchFocusNextField,
            HostEffect::SearchFocusPreviousField,
        ]
    );
}

#[test]
fn command_args_should_parse_through_try_from_contract() {
    assert_eq!(
        InsertTextArgs::try_from(CommandArgs::new().with("text", "hi")).unwrap(),
        InsertTextArgs {
            text: "hi".to_string()
        }
    );
    assert_eq!(
        ReplaceSelectionArgs::try_from(CommandArgs::new().with("text", "bye")).unwrap(),
        ReplaceSelectionArgs {
            text: "bye".to_string()
        }
    );
    assert_eq!(
        MoveSelectionArgs::try_from(
            CommandArgs::new()
                .with("direction", "right")
                .with("motion", "line-edge")
                .with("extend", "true")
        )
        .unwrap(),
        MoveSelectionArgs {
            direction: MovementDirection::Next,
            motion: Motion::ByUnit(MovementUnit::LineEdge),
            extend: true,
        }
    );

    // page-step 携带 lines。
    assert_eq!(
        MoveSelectionArgs::try_from(
            CommandArgs::new()
                .with("direction", "next")
                .with("motion", "page-step")
                .with("lines", "30")
        )
        .unwrap(),
        MoveSelectionArgs {
            direction: MovementDirection::Next,
            motion: Motion::PageStep { lines: 30 },
            extend: false,
        }
    );

    // page-step 缺 lines → 报错。
    assert!(matches!(
        MoveSelectionArgs::try_from(
            CommandArgs::new()
                .with("direction", "next")
                .with("motion", "page-step")
        ),
        Err(CommandError::InvalidArgs(_))
    ));

    // 序列化 round-trip：PageStep 自带 lines。
    let original = MoveSelectionArgs {
        direction: MovementDirection::Previous,
        motion: Motion::PageStep { lines: 25 },
        extend: true,
    };
    let serialized: CommandArgs = original.into();
    assert_eq!(serialized.get("motion"), Some("page-step"));
    assert_eq!(serialized.get("lines"), Some("25"));
    assert_eq!(MoveSelectionArgs::try_from(serialized).unwrap(), original);

    assert!(NoArgs::try_from(CommandArgs::new()).is_ok());
    assert!(matches!(
        InsertTextArgs::try_from(CommandArgs::new()),
        Err(CommandError::InvalidArgs(_))
    ));
    assert!(matches!(
        NoArgs::try_from(CommandArgs::new().with("text", "x")),
        Err(CommandError::InvalidArgs(_))
    ));
}

#[test]
fn executor_should_drain_queue_and_allow_handlers_to_enqueue_followups() {
    let (mut workspace, mut views, _) = setup("");
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut registry = CommandRegistry::new();

    {
        let seen = Rc::clone(&seen);
        registry
            .register(
                Command::new(command_id("test.first"), "第一步"),
                Box::new(move |context, _args| {
                    seen.borrow_mut().push("first");
                    context
                        .queue
                        .dispatch(command_id("test.second"), CommandArgs::new());
                    Ok(Default::default())
                }),
            )
            .unwrap();
    }
    {
        let seen = Rc::clone(&seen);
        registry
            .register(
                Command::new(command_id("test.second"), "第二步"),
                Box::new(move |_context, _args| {
                    seen.borrow_mut().push("second");
                    Ok(Default::default())
                }),
            )
            .unwrap();
    }

    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![("test.first", CommandArgs::new())],
    )
    .unwrap();

    assert_eq!(seen.borrow().as_slice(), ["first", "second"]);
}

#[test]
fn builtin_editor_commands_should_edit_active_view_buffer_and_sync_selection() {
    let (mut workspace, mut views, buffer_id) = setup("abc");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![(editor::INSERT_TEXT, CommandArgs::new().with("text", "你"))],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "你abc");
    assert_eq!(
        views.active_view().unwrap().selection().primary().head(),
        byte("你".len())
    );

    *views.active_view_mut().unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::new(byte("你".len()), byte("你a".len()))]);
    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![(
            editor::REPLACE_SELECTION,
            CommandArgs::new().with("text", "Z"),
        )],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "你Zbc");

    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![(editor::DELETE_BACKWARD, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "你bc");

    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![(editor::DELETE_FORWARD, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "你c");
}

#[test]
fn newline_indent_and_outdent_commands_should_edit_active_view_buffer() {
    let (mut workspace, mut views, buffer_id) = setup("a");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![(editor::INSERT_NEWLINE, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "\na");

    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![(editor::INDENT, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "\n    a");

    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![(editor::OUTDENT, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "\na");
}

#[test]
fn select_all_undo_and_redo_should_roundtrip_text_and_view_selection() {
    let (mut workspace, mut views, buffer_id) = setup("abc");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![
            (editor::SELECT_ALL, CommandArgs::new()),
            (
                editor::REPLACE_SELECTION,
                CommandArgs::new().with("text", "xyz"),
            ),
        ],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "xyz");
    assert_eq!(
        views.active_view().unwrap().selection().primary().head(),
        byte(3)
    );

    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![(editor::UNDO, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "abc");
    assert_eq!(
        views.active_view().unwrap().selection().primary().range(),
        Selection::new(byte(0), byte(3)).range()
    );

    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![(editor::REDO, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "xyz");
    assert_eq!(
        views.active_view().unwrap().selection().primary().head(),
        byte(3)
    );
}

#[test]
fn movement_commands_should_update_active_view_selection() {
    let (mut workspace, mut views, _) = setup("hello world");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    // 字符级右移 —— 现在通过 typed builder 拼出，不再有 editor.move_right 命令面。
    let (id, args) = editor::move_selection(MovementDirection::Next, MovementUnit::Grapheme, false);
    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![(id.as_str(), args)],
    )
    .unwrap();
    assert_eq!(
        views.active_view().unwrap().selection().primary().head(),
        byte(1)
    );

    // 扩展选区右移一格。
    let (id, args) = editor::move_selection(MovementDirection::Next, MovementUnit::Grapheme, true);
    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![(id.as_str(), args)],
    )
    .unwrap();
    assert_eq!(
        *views.active_view().unwrap().selection().primary(),
        Selection::new(byte(1), byte(2))
    );

    // 按词右移。
    let (id, args) = editor::move_selection(MovementDirection::Next, MovementUnit::Word, false);
    run(
        &registry,
        &mut workspace,
        &mut views,
        vec![(id.as_str(), args)],
    )
    .unwrap();
    assert_eq!(
        views.active_view().unwrap().selection().primary().head(),
        byte(5)
    );
}

#[test]
fn editor_default_keymap_should_include_line_and_page_movement() {
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);
    let text_edit = text_edit_context();

    let (_, home_args) =
        editor::move_selection(MovementDirection::Previous, MovementUnit::LineEdge, false);
    assert_eq!(
        keymap.resolve(&[key("home")], &text_edit),
        KeymapResolution::Matched {
            command: command_id(editor::MOVE_SELECTION),
            args: home_args,
        }
    );

    let (_, shift_end_args) =
        editor::move_selection(MovementDirection::Next, MovementUnit::LineEdge, true);
    assert_eq!(
        keymap.resolve(&[key("shift-end")], &text_edit),
        KeymapResolution::Matched {
            command: command_id(editor::MOVE_SELECTION),
            args: shift_end_args,
        }
    );

    let (_, up_args) = editor::move_selection(MovementDirection::Previous, Motion::LineStep, false);
    assert_eq!(
        keymap.resolve(&[key("up")], &text_edit),
        KeymapResolution::Matched {
            command: command_id(editor::MOVE_SELECTION),
            args: up_args,
        }
    );

    let (_, shift_down_args) =
        editor::move_selection(MovementDirection::Next, Motion::LineStep, true);
    assert_eq!(
        keymap.resolve(&[key("shift-down")], &text_edit),
        KeymapResolution::Matched {
            command: command_id(editor::MOVE_SELECTION),
            args: shift_down_args,
        }
    );

    // PageStep：默认 lines=20，序列化时一并写入 args。
    let (_, pagedown_args) = editor::move_selection(
        MovementDirection::Next,
        Motion::PageStep { lines: 20 },
        false,
    );
    assert_eq!(
        keymap.resolve(&[key("pagedown")], &text_edit),
        KeymapResolution::Matched {
            command: command_id(editor::MOVE_SELECTION),
            args: pagedown_args,
        }
    );
}

#[test]
fn editor_default_keymap_should_include_newline_indent_and_outdent() {
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);
    let text_edit = text_edit_context();
    let multiline = multiline_text_edit_context();
    let composing = composing_text_edit_context();

    assert_eq!(
        keymap.resolve(&[key("enter")], &multiline),
        KeymapResolution::Matched {
            command: command_id(editor::INSERT_NEWLINE),
            args: CommandArgs::new(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("return")], &multiline),
        KeymapResolution::Matched {
            command: command_id(editor::INSERT_NEWLINE),
            args: CommandArgs::new(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("enter")], &text_edit),
        KeymapResolution::NoMatch
    );
    assert_eq!(
        keymap.resolve(&[key("enter")], &composing),
        KeymapResolution::Matched {
            command: command_id(editor::IME_CONFIRM),
            args: CommandArgs::new(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("tab")], &text_edit),
        KeymapResolution::NoMatch
    );
    assert_eq!(
        keymap.resolve(&[key("shift-tab")], &text_edit),
        KeymapResolution::NoMatch
    );
    assert_eq!(
        keymap.resolve(&[key("tab")], &multiline),
        KeymapResolution::Matched {
            command: command_id(editor::INDENT),
            args: CommandArgs::new(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("shift-tab")], &multiline),
        KeymapResolution::Matched {
            command: command_id(editor::OUTDENT),
            args: CommandArgs::new(),
        }
    );
}

#[test]
fn file_tree_keymap_should_share_chords_with_editor_by_context() {
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);
    file_tree::install(&mut registry, &mut keymap);

    let text_edit = text_edit_context();
    let file_tree_navigate = file_tree_context(FileTreeKeyMode::Navigate);
    let pending_name = [
        KeyContext::text_edit(false, false),
        KeyContext::file_tree(FileTreeKeyMode::PendingName),
        KeyContext::global(),
    ];
    let composing_pending_name = [
        KeyContext::text_edit(false, true),
        KeyContext::file_tree(FileTreeKeyMode::PendingName),
        KeyContext::global(),
    ];

    let (_, editor_up_args) =
        editor::move_selection(MovementDirection::Previous, Motion::LineStep, false);
    assert_eq!(
        keymap.resolve(&[key("up")], &text_edit),
        KeymapResolution::Matched {
            command: command_id(editor::MOVE_SELECTION),
            args: editor_up_args,
        }
    );
    assert_eq!(
        keymap.resolve(&[key("up")], &file_tree_navigate),
        KeymapResolution::Matched {
            command: command_id(file_tree::MOVE_SELECTION),
            args: file_tree::MoveSelectionArgs { delta: -1 }.into(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("enter")], &pending_name),
        KeymapResolution::Matched {
            command: command_id(file_tree::COMMIT_NEW_ENTRY),
            args: CommandArgs::new(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("enter")], &composing_pending_name),
        KeymapResolution::Matched {
            command: command_id(editor::IME_CONFIRM),
            args: CommandArgs::new(),
        }
    );
}

#[test]
fn file_tree_commands_should_emit_host_effects() {
    let (mut workspace, mut views, _) = setup("");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    file_tree::install(&mut registry, &mut keymap);

    let effects = run_and_collect_effects(
        &registry,
        &mut workspace,
        &mut views,
        vec![
            (
                file_tree::MOVE_SELECTION,
                file_tree::MoveSelectionArgs { delta: 1 }.into(),
            ),
            (file_tree::BEGIN_NEW_ENTRY, CommandArgs::new()),
            (file_tree::CANCEL_NEW_ENTRY, CommandArgs::new()),
        ],
    )
    .unwrap();

    assert_eq!(
        effects,
        vec![
            HostEffect::FileTreeMoveSelection(1),
            HostEffect::FileTreeBeginNewEntry,
            HostEffect::FileTreeCancelNewEntry,
        ]
    );
}

#[test]
fn keymap_should_resolve_prefixes_and_prioritized_contexts() {
    let mut keymap = Keymap::new();
    keymap.bind(KeyBinding {
        sequence: vec![key("ctrl+x"), key("s")],
        command: command_id("file.save"),
        args: CommandArgs::new(),
        context: KeyBindingContext::text_edit(),
    });
    keymap.bind(KeyBinding {
        sequence: vec![key("ctrl+x"), key("c")],
        command: command_id("editor.copy"),
        args: CommandArgs::new().with("kind", "copy"),
        context: KeyBindingContext::global(),
    });
    keymap.bind(KeyBinding {
        sequence: vec![key("ctrl+x"), key("c")],
        command: command_id("editor.cancel"),
        args: CommandArgs::new().with("kind", "cancel"),
        context: KeyBindingContext::file_tree(FileTreeKeyMode::Navigate),
    });
    assert!(matches!(
        keymap.try_bind(KeyBinding {
            sequence: vec![key("ctrl+x"), key("c")],
            command: command_id("editor.duplicate_copy"),
            args: CommandArgs::new(),
            context: KeyBindingContext::global(),
        }),
        Err(CommandError::DuplicateKeyBinding { .. })
    ));

    let editor_contexts = text_edit_context();
    let global_contexts = global_context();
    let file_tree_contexts = file_tree_context(FileTreeKeyMode::Navigate);
    assert_eq!(
        keymap.resolve(&[key("ctrl+x")], &editor_contexts),
        KeymapResolution::Pending
    );
    assert_eq!(
        keymap.resolve(&[key("ctrl+x"), key("s")], &global_contexts),
        KeymapResolution::NoMatch
    );
    assert_eq!(
        keymap.resolve(&[key("ctrl+x"), key("s")], &editor_contexts),
        KeymapResolution::Matched {
            command: command_id("file.save"),
            args: CommandArgs::new(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("ctrl+x"), key("c")], &global_contexts),
        KeymapResolution::Matched {
            command: command_id("editor.copy"),
            args: CommandArgs::new().with("kind", "copy"),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("ctrl+x"), key("c")], &file_tree_contexts),
        KeymapResolution::Matched {
            command: command_id("editor.cancel"),
            args: CommandArgs::new().with("kind", "cancel"),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("ctrl+z")], &editor_contexts),
        KeymapResolution::NoMatch
    );
}

#[test]
fn keymap_should_reject_overlapping_but_allow_disjoint_text_edit_contexts() {
    let mut keymap = Keymap::new();
    keymap.bind(KeyBinding {
        sequence: vec![key("enter")],
        command: command_id("editor.a"),
        args: CommandArgs::new(),
        context: KeyBindingContext::text_edit(),
    });

    // text_edit 与 text_edit_multiline 的 composition 都是 Inactive（后者只多了
    // requires_newline 过滤），同一序列会被同一运行时上下文同时命中——重叠即冲突。
    assert!(matches!(
        keymap.try_bind(KeyBinding {
            sequence: vec![key("enter")],
            command: command_id("editor.b"),
            args: CommandArgs::new(),
            context: KeyBindingContext::text_edit_multiline(),
        }),
        Err(CommandError::DuplicateKeyBinding { .. })
    ));

    // composition 互斥（Inactive vs Active）——上下文不重叠，同一序列可并存。
    assert!(
        keymap
            .try_bind(KeyBinding {
                sequence: vec![key("enter")],
                command: command_id("editor.c"),
                args: CommandArgs::new(),
                context: KeyBindingContext::text_edit_composition(),
            })
            .is_ok()
    );
}
