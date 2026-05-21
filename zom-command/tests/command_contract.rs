use std::cell::RefCell;
use std::rc::Rc;

use zom_command::commands::editor::{
    self, InsertTextArgs, MoveSelectionArgs, ReplaceSelectionArgs,
};
use zom_command::{
    Command, CommandArgs, CommandContext, CommandError, CommandExecutor, CommandId, CommandQueue,
    CommandRegistry, EffectQueue, KeyBinding, KeyChord, Keymap, KeymapResolution, NoArgs,
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

fn text(workspace: &Workspace, buffer_id: BufferId) -> String {
    workspace
        .buffer(buffer_id)
        .unwrap()
        .buffer()
        .text()
        .into_owned()
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

    let (_, home_args) =
        editor::move_selection(MovementDirection::Previous, MovementUnit::LineEdge, false);
    assert_eq!(
        keymap.resolve(&[key("home")], &[]),
        KeymapResolution::Matched {
            command: command_id(editor::MOVE_SELECTION),
            args: home_args,
        }
    );

    let (_, shift_end_args) =
        editor::move_selection(MovementDirection::Next, MovementUnit::LineEdge, true);
    assert_eq!(
        keymap.resolve(&[key("shift-end")], &[]),
        KeymapResolution::Matched {
            command: command_id(editor::MOVE_SELECTION),
            args: shift_end_args,
        }
    );

    let (_, up_args) = editor::move_selection(MovementDirection::Previous, Motion::LineStep, false);
    assert_eq!(
        keymap.resolve(&[key("up")], &[]),
        KeymapResolution::Matched {
            command: command_id(editor::MOVE_SELECTION),
            args: up_args,
        }
    );

    let (_, shift_down_args) =
        editor::move_selection(MovementDirection::Next, Motion::LineStep, true);
    assert_eq!(
        keymap.resolve(&[key("shift-down")], &[]),
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
        keymap.resolve(&[key("pagedown")], &[]),
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

    assert_eq!(
        keymap.resolve(&[key("enter")], &[]),
        KeymapResolution::Matched {
            command: command_id(editor::INSERT_NEWLINE),
            args: CommandArgs::new(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("return")], &[]),
        KeymapResolution::Matched {
            command: command_id(editor::INSERT_NEWLINE),
            args: CommandArgs::new(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("tab")], &[]),
        KeymapResolution::Matched {
            command: command_id(editor::INDENT),
            args: CommandArgs::new(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("shift-tab")], &[]),
        KeymapResolution::Matched {
            command: command_id(editor::OUTDENT),
            args: CommandArgs::new(),
        }
    );
}

#[test]
fn keymap_should_resolve_prefixes_matches_contexts_and_overrides() {
    let mut keymap = Keymap::new();
    keymap.bind(KeyBinding {
        sequence: vec![key("ctrl+x"), key("s")],
        command: command_id("file.save"),
        args: CommandArgs::new(),
        when: Some("editor".to_string()),
    });
    keymap.bind(KeyBinding {
        sequence: vec![key("ctrl+x"), key("c")],
        command: command_id("editor.copy"),
        args: CommandArgs::new().with("kind", "copy"),
        when: None,
    });
    keymap.bind(KeyBinding {
        sequence: vec![key("ctrl+x"), key("c")],
        command: command_id("editor.cancel"),
        args: CommandArgs::new().with("kind", "cancel"),
        when: None,
    });

    let editor_contexts = vec!["editor".to_string()];
    assert_eq!(
        keymap.resolve(&[key("ctrl+x")], &editor_contexts),
        KeymapResolution::Pending
    );
    assert_eq!(
        keymap.resolve(&[key("ctrl+x"), key("s")], &[]),
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
        keymap.resolve(&[key("ctrl+x"), key("c")], &[]),
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
