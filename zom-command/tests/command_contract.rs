use std::cell::RefCell;
use std::rc::Rc;

use zom_command::commands::{
    diagnostics,
    editor::{self, InsertTextArgs, MoveCaretArgs, ReplaceSelectionArgs},
    file_tree, project_picker,
    search::{self, file as search_file, project as search_project},
    settings,
};
use zom_command::{
    BubbleKind, Command, CommandArgs, CommandContext, CommandError, CommandId, CommandQueue,
    CommandRegistry, DismissScope, DismissStacks, EditTarget, EffectQueue, FileTreeKeyMode,
    HostEffect, KeyBinding, KeyBindingContext, KeyChord, KeyContext, Keymap, KeymapResolution,
    NoArgs, PanelKind, SearchOption, SettingsChangeRequest,
};
use zom_engine::{
    Buffer, BufferConfig, ByteOffset, Motion, MovementDirection, MovementUnit, Selection,
    SelectionSet, TransactionMergePolicy,
};
use zom_workspace::view::{ViewId, ViewSet, VisualAffinity, VisualPosition};
use zom_workspace::{BufferId, Workspace};

mod support;

use support::MockClipboard;

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
    [KeyContext::search_bar()]
}

fn file_tree_context(mode: FileTreeKeyMode) -> [KeyContext; 1] {
    [KeyContext::file_tree(mode)]
}

fn byte(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

/// 给契约测试用的极简 setup：开一个 buffer + 单 view，返回 (workspace, views, buffer_id, view_id)。
///
/// `view_id` 是测试运行命令时要传给 [`CommandContext::active_view_id`] 的活动视图。
/// 旧版本 ViewSet 自动维护"哪个 view 活动"；现在那条职责交给上层（桌面端的 WorkspaceSession，
/// 契约测试这一侧由测试自己拿着 view_id）。
fn setup(text: &str) -> (Workspace, ViewSet, BufferId, ViewId) {
    let mut workspace = Workspace::new();
    let buffer_id = workspace.open_text(None, text).unwrap();
    let version = workspace.buffer(buffer_id).unwrap().buffer().version();
    let mut views = ViewSet::new();
    let view_id = views.open_edit_view(buffer_id, version);
    (workspace, views, buffer_id, view_id)
}

fn run(
    registry: &CommandRegistry,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    active_view_id: ViewId,
    calls: Vec<(&str, CommandArgs)>,
) -> Result<(), CommandError> {
    let mut queue = CommandQueue::new();
    for (id, args) in calls {
        queue.enqueue(command_id(id), args);
    }

    let mut effects = EffectQueue::new();
    let mut clipboard = MockClipboard::new();
    let mut dismiss = DismissStacks::new();
    let mut context = CommandContext {
        workspace,
        views,
        active_view_id: Some(active_view_id),
        focused_field: None,
        queue: &mut queue,
        effects: &mut effects,
        clipboard: &mut clipboard,
        dismiss: &mut dismiss,
        edit_merge_policy: TransactionMergePolicy::Never,
    };
    zom_command::run(registry, &mut context)
}

/// 与 [`run`] 同形，但接收外部 `MockClipboard` 供测试断言其内容。
fn run_with_clipboard(
    registry: &CommandRegistry,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    active_view_id: ViewId,
    clipboard: &mut MockClipboard,
    calls: Vec<(&str, CommandArgs)>,
) -> Result<(), CommandError> {
    let mut queue = CommandQueue::new();
    for (id, args) in calls {
        queue.enqueue(command_id(id), args);
    }

    let mut effects = EffectQueue::new();
    let mut dismiss = DismissStacks::new();
    let mut context = CommandContext {
        workspace,
        views,
        active_view_id: Some(active_view_id),
        focused_field: None,
        queue: &mut queue,
        effects: &mut effects,
        clipboard,
        dismiss: &mut dismiss,
        edit_merge_policy: TransactionMergePolicy::Never,
    };
    zom_command::run(registry, &mut context)
}

fn run_and_collect_effects(
    registry: &CommandRegistry,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    active_view_id: ViewId,
    calls: Vec<(&str, CommandArgs)>,
) -> Result<Vec<HostEffect>, CommandError> {
    let mut queue = CommandQueue::new();
    for (id, args) in calls {
        queue.enqueue(command_id(id), args);
    }

    let mut effects = EffectQueue::new();
    let mut clipboard = MockClipboard::new();
    let mut dismiss = DismissStacks::new();
    let mut context = CommandContext {
        workspace,
        views,
        active_view_id: Some(active_view_id),
        focused_field: None,
        queue: &mut queue,
        effects: &mut effects,
        clipboard: &mut clipboard,
        dismiss: &mut dismiss,
        edit_merge_policy: TransactionMergePolicy::Never,
    };
    zom_command::run(registry, &mut context)?;
    Ok(effects.drain())
}

fn text(workspace: &Workspace, buffer_id: BufferId) -> String {
    buffer_text(workspace.buffer(buffer_id).unwrap().buffer())
}

fn buffer_text(buffer: &zom_engine::Buffer) -> String {
    buffer
        .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
        .unwrap()
        .into_text()
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
        (PanelKind::FileTree.toggle_command_id(), "文件树"),
        (PanelKind::VersionControl.toggle_command_id(), "版本管理"),
        (PanelKind::Outline.toggle_command_id(), "大纲"),
        (search_file::ACTIVATE, "查找"),
        (search_project::PROJECT_ACTIVATE, "项目搜索"),
        (search_file::TOGGLE_CASE_SENSITIVE, "区分大小写"),
        (search_file::TOGGLE_WHOLE_WORD, "全词匹配"),
        (search_file::TOGGLE_REGEX, "正则表达式"),
        (search_file::FIND_PREVIOUS, "上一个"),
        (search_file::FIND_NEXT, "下一个"),
        (search_file::REPLACE_NEXT, "替换下一个"),
        (search_file::REPLACE_ALL, "全部替换"),
        (PanelKind::Terminal.toggle_command_id(), "终端"),
        (PanelKind::Debug.toggle_command_id(), "调试"),
        (PanelKind::KeyboardShortcuts.toggle_command_id(), "快捷键"),
    ];

    for (id, title) in registered_titles {
        let id = command_id(id);
        let command = registry.command(&id).expect("命令必须注册");
        assert_eq!(command.title, title);
    }

    assert!(
        keymap
            .format_shortcuts_for(&command_id(settings::OPEN))
            .is_some()
    );
    assert!(
        keymap
            .format_shortcuts_for(&command_id(PanelKind::FileTree.toggle_command_id()))
            .is_some()
    );
    assert!(
        keymap
            .format_shortcuts_for(&command_id(search_file::TOGGLE_CASE_SENSITIVE))
            .is_some()
    );
    assert!(
        keymap
            .format_shortcuts_for(&command_id(search_file::REPLACE_ALL))
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
        visible.len() >= 10,
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
            keymap.format_shortcuts_for(&command.id).is_some(),
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
    let (mut workspace, mut views, _, view_id) = setup("");

    let effects = run_and_collect_effects(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![
            (search_file::ACTIVATE, CommandArgs::new()),
            (search_file::TOGGLE_CASE_SENSITIVE, CommandArgs::new()),
            (search_file::TOGGLE_WHOLE_WORD, CommandArgs::new()),
            (search_file::TOGGLE_REGEX, CommandArgs::new()),
            (search_file::FIND_PREVIOUS, CommandArgs::new()),
            (search_file::FIND_NEXT, CommandArgs::new()),
            (search_file::REPLACE_NEXT, CommandArgs::new()),
            (search_file::REPLACE_ALL, CommandArgs::new()),
            (search_file::DISMISS, CommandArgs::new()),
            (search_file::CONFIRM_MATCH, CommandArgs::new()),
        ],
    )
    .unwrap();

    assert_eq!(
        effects,
        vec![
            HostEffect::SearchActivate,
            HostEffect::SearchToggleOption(SearchOption::CaseSensitive),
            HostEffect::SearchToggleOption(SearchOption::WholeWord),
            HostEffect::SearchToggleOption(SearchOption::Regex),
            HostEffect::SearchFindPrevious,
            HostEffect::SearchFindNext,
            HostEffect::SearchReplaceNext,
            HostEffect::SearchReplaceAll,
            HostEffect::SearchDismiss,
            HostEffect::SearchConfirmMatch,
        ]
    );
}

#[test]
fn settings_ui_commands_should_emit_host_effects() {
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    settings::install(&mut registry, &mut keymap);
    let (mut workspace, mut views, _, view_id) = setup("");

    let effects = run_and_collect_effects(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![
            (settings::OPEN_TOML, CommandArgs::new()),
            (
                settings::APPLY_CHANGE,
                settings::SettingsChangeArgs {
                    change: SettingsChangeRequest::AdjustEditorFont(1),
                }
                .into(),
            ),
        ],
    )
    .unwrap();

    assert_eq!(
        effects,
        vec![
            HostEffect::SettingsOpenToml,
            HostEffect::SettingsApplyChange(SettingsChangeRequest::AdjustEditorFont(1)),
        ]
    );
}

#[test]
fn save_without_file_path_should_emit_error_bubble() {
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);
    let (mut workspace, mut views, _, view_id) = setup("");

    let effects = run_and_collect_effects(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::SAVE, CommandArgs::new())],
    )
    .unwrap();

    assert_eq!(effects.len(), 1);
    let HostEffect::ShowBubble(request) = &effects[0] else {
        panic!("保存失败应该显示气泡，实际为 {:?}", effects[0]);
    };
    assert_eq!(request.kind, BubbleKind::Error);
    assert_eq!(request.dedupe_key.as_deref(), Some("editor.save"));
    assert_eq!(request.ttl_ms, Some(2400));
    assert!(
        request
            .message
            .starts_with("保存失败：缓冲区未绑定文件路径："),
        "unexpected save error bubble: {}",
        request.message
    );
}

#[test]
fn search_activate_shortcut_should_be_available_in_text_edit_and_search_contexts() {
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    search::install(&mut registry, &mut keymap);
    // 搜索快捷键限定在 text_edit 内；纯空态没活动文件时不响应。
    // 同时也可在 search_panel 内触发——按一次开 bar、再按一次收起 bar 的口径。
    let editor = text_edit_context();
    let search_field = [
        KeyContext::search_bar(),
        KeyContext::text_edit(false, false),
    ];

    for contexts in [&editor[..], &search_field[..]] {
        assert_eq!(
            keymap.resolve(&[key("mod f")], contexts),
            KeymapResolution::Matched {
                command: command_id(search_file::ACTIVATE),
                args: CommandArgs::new(),
            }
        );
    }
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
            command: command_id(search_file::FOCUS_NEXT_FIELD),
            args: CommandArgs::new(),
        }
    );
    assert_eq!(
        keymap.resolve(&[key("shift tab")], &search_panel),
        KeymapResolution::Matched {
            command: command_id(search_file::FOCUS_PREVIOUS_FIELD),
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

    let (mut workspace, mut views, _, view_id) = setup("");
    let effects = run_and_collect_effects(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![
            (search_file::FOCUS_NEXT_FIELD, CommandArgs::new()),
            (search_file::FOCUS_PREVIOUS_FIELD, CommandArgs::new()),
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
        MoveCaretArgs::try_from(
            CommandArgs::new()
                .with("direction", "right")
                .with("motion", "line-edge")
                .with("extend", "true")
        )
        .unwrap(),
        MoveCaretArgs {
            direction: MovementDirection::Next,
            motion: Motion::ByUnit(MovementUnit::LineEdge),
            extend: true,
        }
    );

    // page-step 携带 lines。
    assert_eq!(
        MoveCaretArgs::try_from(
            CommandArgs::new()
                .with("direction", "next")
                .with("motion", "page-step")
                .with("lines", "30")
        )
        .unwrap(),
        MoveCaretArgs {
            direction: MovementDirection::Next,
            motion: Motion::PageStep { lines: 30 },
            extend: false,
        }
    );

    // page-step 缺 lines → 报错。
    assert!(matches!(
        MoveCaretArgs::try_from(
            CommandArgs::new()
                .with("direction", "next")
                .with("motion", "page-step")
        ),
        Err(CommandError::InvalidArgs(_))
    ));

    // 序列化 round-trip：PageStep 自带 lines。
    let original = MoveCaretArgs {
        direction: MovementDirection::Previous,
        motion: Motion::PageStep { lines: 25 },
        extend: true,
    };
    let serialized: CommandArgs = original.into();
    assert_eq!(serialized.get("motion"), Some("page-step"));
    assert_eq!(serialized.get("lines"), Some("25"));
    assert_eq!(MoveCaretArgs::try_from(serialized).unwrap(), original);

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
    let (mut workspace, mut views, _, view_id) = setup("");
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
                        .enqueue(command_id("test.second"), CommandArgs::new());
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
        view_id,
        vec![("test.first", CommandArgs::new())],
    )
    .unwrap();

    assert_eq!(seen.borrow().as_slice(), ["first", "second"]);
}

#[test]
fn builtin_editor_commands_should_edit_active_view_buffer_and_sync_selection() {
    let (mut workspace, mut views, buffer_id, view_id) = setup("abc");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::INSERT_TEXT, CommandArgs::new().with("text", "你"))],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "你abc");
    assert_eq!(
        views
            .edit_view(view_id)
            .unwrap()
            .selection()
            .primary()
            .head(),
        byte("你".len())
    );

    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::new(byte("你".len()), byte("你a".len()))]);
    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
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
        view_id,
        vec![(
            editor::DELETE,
            CommandArgs::new().with("direction", "previous"),
        )],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "你bc");

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::DELETE, CommandArgs::new().with("direction", "next"))],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "你c");
}

#[test]
fn delete_with_word_motion_removes_previous_word() {
    let (mut workspace, mut views, buffer_id, view_id) = setup("hello world");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    // caret 落到行尾。
    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::caret(byte("hello world".len()))]);

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(
            editor::DELETE,
            CommandArgs::new()
                .with("direction", "previous")
                .with("motion", "word"),
        )],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "hello ");
    assert_eq!(
        views
            .edit_view(view_id)
            .unwrap()
            .selection()
            .primary()
            .head(),
        byte("hello ".len()),
    );
}

#[test]
fn delete_with_word_motion_removes_next_word() {
    let (mut workspace, mut views, buffer_id, view_id) = setup("hello world");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::caret(ByteOffset::ZERO)]);

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(
            editor::DELETE,
            CommandArgs::new()
                .with("direction", "next")
                .with("motion", "word"),
        )],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), " world");
}

#[test]
fn delete_with_unknown_motion_should_error() {
    let (mut workspace, mut views, _buffer_id, view_id) = setup("abc");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);
    let err = run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(
            editor::DELETE,
            CommandArgs::new()
                .with("direction", "previous")
                .with("motion", "line-step"),
        )],
    )
    .unwrap_err();
    assert!(
        matches!(err, CommandError::InvalidArgs(_)),
        "line-step / page-step 不应被 DeleteArgs 接受，实际：{err:?}",
    );
}

#[test]
fn delete_without_direction_deletes_only_non_empty_selection() {
    // direction 缺席 = caret 不动、只删非空 selection。
    let (mut workspace, mut views, buffer_id, view_id) = setup("hello world");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    // 选中 "hello"。
    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::new(ByteOffset::ZERO, byte("hello".len()))]);

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::DELETE, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), " world");
    assert_eq!(
        views
            .edit_view(view_id)
            .unwrap()
            .selection()
            .primary()
            .head(),
        ByteOffset::ZERO,
    );
}

#[test]
fn delete_without_direction_is_noop_on_caret() {
    // 只有 caret、没有非空选区时 direction=None 等价 no-op，文本与 caret 都不动。
    let (mut workspace, mut views, buffer_id, view_id) = setup("hello world");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    let caret_at = byte("hello".len());
    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::caret(caret_at)]);

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::DELETE, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "hello world");
    assert_eq!(
        views
            .edit_view(view_id)
            .unwrap()
            .selection()
            .primary()
            .head(),
        caret_at,
    );
}

#[test]
fn delete_with_motion_but_no_direction_should_error() {
    // motion 出现但 direction 缺席：歧义（unit 会被忽略），TryFrom 必须报错。
    let (mut workspace, mut views, _buffer_id, view_id) = setup("abc");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);
    let err = run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::DELETE, CommandArgs::new().with("motion", "word"))],
    )
    .unwrap_err();
    assert!(
        matches!(err, CommandError::InvalidArgs(_)),
        "motion 不带 direction 应报 InvalidArgs，实际：{err:?}",
    );
}

#[test]
fn default_keymap_binds_delete_variants() {
    // 默认 keymap：backspace / delete / alt backspace / alt delete 都落到 DELETE，
    // 用 direction × motion args 区分四种行为。
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    let context = text_edit_context();
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("backspace", "previous", None),
        ("delete", "next", None),
        ("alt backspace", "previous", Some("word")),
        ("alt delete", "next", Some("word")),
    ];
    for (chord, direction, motion) in cases {
        match keymap.resolve(&[key(chord)], &context) {
            KeymapResolution::Matched { command, args } => {
                assert_eq!(command, command_id(editor::DELETE), "{chord}");
                assert_eq!(args.get("direction"), Some(*direction), "{chord} direction");
                assert_eq!(args.get("motion"), *motion, "{chord} motion");
            }
            other => panic!("{chord} 应命中 DELETE，实际：{other:?}"),
        }
    }
}

#[test]
fn newline_indent_and_outdent_commands_should_edit_active_view_buffer() {
    let (mut workspace, mut views, buffer_id, view_id) = setup("a");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::INSERT_NEWLINE, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "\na");

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::INDENT, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "\n    a");

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::OUTDENT, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "\na");
}

#[test]
fn select_all_undo_and_redo_should_roundtrip_text_and_view_selection() {
    let (mut workspace, mut views, buffer_id, view_id) = setup("abc");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
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
        views
            .edit_view(view_id)
            .unwrap()
            .selection()
            .primary()
            .head(),
        byte(3)
    );

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::UNDO, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "abc");
    assert_eq!(
        views
            .edit_view(view_id)
            .unwrap()
            .selection()
            .primary()
            .range(),
        Selection::new(byte(0), byte(3)).range()
    );

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::REDO, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "xyz");
    assert_eq!(
        views
            .edit_view(view_id)
            .unwrap()
            .selection()
            .primary()
            .head(),
        byte(3)
    );
}

#[test]
fn clear_selection_should_collapse_each_selection_to_caret_at_head() {
    let (mut workspace, mut views, _, view_id) = setup("hello world");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    // 先扩出非空选区。
    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::SELECT_ALL, CommandArgs::new())],
    )
    .unwrap();
    assert!(
        !views
            .edit_view(view_id)
            .unwrap()
            .selection()
            .primary()
            .is_caret()
    );

    // CLEAR_SELECTION 把每条选区塌成 caret，head 不动，并通知宿主取消鼠标拖选会话。
    let effects = run_and_collect_effects(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::CLEAR_SELECTION, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(effects, vec![HostEffect::EditorCancelPointerSelection]);
    let primary = views.edit_view(view_id).unwrap().selection().primary();
    assert!(primary.is_caret(), "clear_selection 必须留下纯 caret");
    assert_eq!(primary.head(), byte("hello world".len()));

    // 已是 caret 时再调一次是 no-op，不报错。
    let effects = run_and_collect_effects(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::CLEAR_SELECTION, CommandArgs::new())],
    )
    .unwrap();
    assert!(effects.is_empty());
    let primary = views.edit_view(view_id).unwrap().selection().primary();
    assert!(primary.is_caret());
}

#[test]
fn movement_commands_should_update_active_view_selection() {
    let (mut workspace, mut views, _, view_id) = setup("hello world");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    // 字符级右移 —— 现在通过 typed builder 拼出，不再有 editor.move_right 命令面。
    let (id, args) = editor::move_selection(MovementDirection::Next, MovementUnit::Grapheme, false);
    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(id.as_str(), args)],
    )
    .unwrap();
    assert_eq!(
        views
            .edit_view(view_id)
            .unwrap()
            .selection()
            .primary()
            .head(),
        byte(1)
    );

    // 扩展选区右移一格。
    let (id, args) = editor::move_selection(MovementDirection::Next, MovementUnit::Grapheme, true);
    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(id.as_str(), args)],
    )
    .unwrap();
    assert_eq!(
        *views.edit_view(view_id).unwrap().selection().primary(),
        Selection::new(byte(1), byte(2))
    );

    // 按词右移。
    let (id, args) = editor::move_selection(MovementDirection::Next, MovementUnit::Word, false);
    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(id.as_str(), args)],
    )
    .unwrap();
    assert_eq!(
        views
            .edit_view(view_id)
            .unwrap()
            .selection()
            .primary()
            .head(),
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
        keymap.resolve(&[key("shift end")], &text_edit),
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
        keymap.resolve(&[key("shift down")], &text_edit),
        KeymapResolution::Matched {
            command: command_id(editor::MOVE_SELECTION),
            args: shift_down_args,
        }
    );

    // PageStep：默认 lines=1，序列化时一并写入 args。
    let (_, pagedown_args) = editor::move_selection(
        MovementDirection::Next,
        Motion::PageStep { lines: 1 },
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
fn copy_with_non_empty_selection_should_write_selected_text_to_clipboard() {
    let (mut workspace, mut views, _buffer_id, view_id) = setup("hello world");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::new(byte(0), byte(5))]);

    let mut clipboard = MockClipboard::new();
    run_with_clipboard(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        &mut clipboard,
        vec![(editor::COPY, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(clipboard.contents(), Some("hello"));
}

#[test]
fn copy_with_multi_non_empty_selections_should_join_pieces_with_newline() {
    let (mut workspace, mut views, _buffer_id, view_id) = setup("abcdef");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    *views.edit_view_mut(view_id).unwrap().selection_mut() = SelectionSet::new(vec![
        Selection::new(byte(0), byte(2)),
        Selection::new(byte(3), byte(5)),
    ]);

    let mut clipboard = MockClipboard::new();
    run_with_clipboard(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        &mut clipboard,
        vec![(editor::COPY, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(clipboard.contents(), Some("ab\nde"));
}

#[test]
fn copy_with_all_carets_should_take_whole_line_with_trailing_newline() {
    // 多行文件，caret 落在第二行——复制结果就是"bar\n"（含 \n）。
    let (mut workspace, mut views, _buffer_id, view_id) = setup("foo\nbar\nbaz\n");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::caret(byte(5))]); // "foo\nb" 之间
    let mut clipboard = MockClipboard::new();
    run_with_clipboard(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        &mut clipboard,
        vec![(editor::COPY, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(clipboard.contents(), Some("bar\n"));
}

#[test]
fn copy_with_multiple_carets_on_same_line_should_dedupe_by_line() {
    // 同一行两个 caret——按 Line 去重，整行只复制一次。
    let (mut workspace, mut views, _buffer_id, view_id) = setup("abcd\nefgh\n");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::caret(byte(0)), Selection::caret(byte(2))]);
    let mut clipboard = MockClipboard::new();
    run_with_clipboard(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        &mut clipboard,
        vec![(editor::COPY, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(clipboard.contents(), Some("abcd\n"));
}

#[test]
fn copy_mixed_caret_and_non_empty_should_ignore_carets() {
    // 二选一：存在非空选区 → 所有 caret 被忽略。
    let (mut workspace, mut views, _buffer_id, view_id) = setup("foo\nbar\n");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    *views.edit_view_mut(view_id).unwrap().selection_mut() = SelectionSet::new(vec![
        Selection::caret(byte(0)),
        Selection::new(byte(4), byte(7)),
    ]);
    let mut clipboard = MockClipboard::new();
    run_with_clipboard(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        &mut clipboard,
        vec![(editor::COPY, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(clipboard.contents(), Some("bar"));
}

#[test]
fn copy_last_line_without_trailing_newline_should_not_synthesize_newline() {
    // 末行无 \n —— 剪贴板拿到的就是无换行的纯文本。
    let (mut workspace, mut views, _buffer_id, view_id) = setup("foo\nbar");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::caret(byte(6))]);
    let mut clipboard = MockClipboard::new();
    run_with_clipboard(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        &mut clipboard,
        vec![(editor::COPY, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(clipboard.contents(), Some("bar"));
}

#[test]
fn cut_with_non_empty_selection_should_write_and_delete() {
    let (mut workspace, mut views, buffer_id, view_id) = setup("hello world");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::new(byte(0), byte(6))]);
    let mut clipboard = MockClipboard::new();
    run_with_clipboard(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        &mut clipboard,
        vec![(editor::CUT, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(clipboard.contents(), Some("hello "));
    assert_eq!(text(&workspace, buffer_id), "world");
}

#[test]
fn cut_with_all_carets_should_delete_whole_lines_with_newline() {
    let (mut workspace, mut views, buffer_id, view_id) = setup("foo\nbar\nbaz\n");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::caret(byte(5))]); // Line 1 = "bar"
    let mut clipboard = MockClipboard::new();
    run_with_clipboard(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        &mut clipboard,
        vec![(editor::CUT, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(clipboard.contents(), Some("bar\n"));
    assert_eq!(text(&workspace, buffer_id), "foo\nbaz\n");
}

#[test]
fn cut_last_line_without_trailing_newline_should_leave_buffer_empty_when_only_line() {
    let (mut workspace, mut views, buffer_id, view_id) = setup("only");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::caret(byte(2))]);
    let mut clipboard = MockClipboard::new();
    run_with_clipboard(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        &mut clipboard,
        vec![(editor::CUT, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(clipboard.contents(), Some("only"));
    assert_eq!(text(&workspace, buffer_id), "");
}

#[test]
fn paste_should_replace_selection_with_clipboard_text_verbatim() {
    let (mut workspace, mut views, buffer_id, view_id) = setup("ab\ncd");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::caret(byte(0))]);
    let mut clipboard = MockClipboard::with_contents("XY\nZ");
    run_with_clipboard(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        &mut clipboard,
        vec![(editor::PASTE, CommandArgs::new())],
    )
    .unwrap();
    // 复制到啥粘贴啥 —— 内嵌的 \n 原样换行。
    assert_eq!(text(&workspace, buffer_id), "XY\nZab\ncd");
}

#[test]
fn paste_with_empty_clipboard_should_be_noop() {
    let (mut workspace, mut views, buffer_id, view_id) = setup("hello");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    let mut clipboard = MockClipboard::new();
    run_with_clipboard(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        &mut clipboard,
        vec![(editor::PASTE, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(text(&workspace, buffer_id), "hello");
}

#[test]
fn insert_newline_should_clear_cached_visual_caret() {
    let (mut workspace, mut views, _buffer_id, view_id) = setup("a");
    let mut registry = CommandRegistry::new();
    let mut throwaway_keymap = Keymap::new();
    editor::install(&mut registry, &mut throwaway_keymap);

    views.edit_view_mut(view_id).unwrap().set_visual_caret(
        Some(VisualPosition {
            byte: ByteOffset::ZERO,
            logical_line: 0,
            subrow: 0,
            column: 0,
            affinity: VisualAffinity::Inside,
        }),
        Some(7),
    );

    run(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![(editor::INSERT_NEWLINE, CommandArgs::new())],
    )
    .unwrap();

    let view = views.edit_view(view_id).unwrap();
    assert_eq!(view.visual_caret(), None);
    assert_eq!(view.goal_column(), None);
}

/// 用单 buffer + 单 selection 模拟"聚焦的内嵌输入框"，跑一条命令并返回剪贴板状态。
///
/// 关键点：当 `focused_field = Some(EditTarget { ... })` 时，handler 走 `focused_field` 分支，
/// **完全不应触碰** `workspace` / `views` 里的主编辑区。
fn run_on_focused_field(
    registry: &CommandRegistry,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    embed_buffer: &mut Buffer,
    embed_selection: &mut SelectionSet,
    clipboard: &mut MockClipboard,
    calls: Vec<(&str, CommandArgs)>,
) -> Result<(), CommandError> {
    let mut queue = CommandQueue::new();
    for (id, args) in calls {
        queue.enqueue(command_id(id), args);
    }

    let mut effects = EffectQueue::new();
    let mut dismiss = DismissStacks::new();
    let mut context = CommandContext {
        workspace,
        views,
        // 此测试 helper 是"焦点在输入框"场景；命令不应触主编辑区，
        // active_view_id 故意置 None 来钉死「focused_field 优先」契约。
        active_view_id: None,
        focused_field: Some(EditTarget {
            buffer: embed_buffer,
            selection: embed_selection,
            wrap_map: None,
            visual_caret: None,
            goal_column: None,
        }),
        queue: &mut queue,
        effects: &mut effects,
        clipboard,
        dismiss: &mut dismiss,
        edit_merge_policy: TransactionMergePolicy::Never,
    };
    zom_command::run(registry, &mut context)
}

#[test]
fn clipboard_commands_should_target_focused_field_not_main_editor() {
    // 主编辑区有自己的内容与选区——一旦剪贴板命令"误闯"到主编辑区，这里就会暴露：
    // 主缓冲会被改、选区会被覆盖、复制的文本会跟它有关。
    let (mut workspace, mut views, main_buffer_id, view_id) = setup("MAIN-EDITOR-TEXT");
    *views.edit_view_mut(view_id).unwrap().selection_mut() =
        SelectionSet::new(vec![Selection::new(
            byte(0),
            byte("MAIN-EDITOR-TEXT".len()),
        )]);
    let main_before = text(&workspace, main_buffer_id);

    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);

    // 内嵌输入框：独立 Buffer + 独立 selection。
    // 模拟 picker / search query / file_tree pending 等任何 `TextTargetOwner` 暴露给 `focused_field` 的形态。
    let mut embed_buffer = Buffer::scratch("embed".to_string(), BufferConfig::default()).unwrap();
    let mut embed_selection = SelectionSet::new(vec![Selection::new(byte(0), byte("embed".len()))]);

    // 1) COPY：写入剪贴板的应该是内嵌内容，不是主编辑区内容。
    let mut clipboard = MockClipboard::new();
    run_on_focused_field(
        &registry,
        &mut workspace,
        &mut views,
        &mut embed_buffer,
        &mut embed_selection,
        &mut clipboard,
        vec![(editor::COPY, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(clipboard.contents(), Some("embed"));
    assert_eq!(
        text(&workspace, main_buffer_id),
        main_before,
        "COPY 不应改动主编辑区文本"
    );

    // 2) PASTE：粘贴目标是内嵌 buffer，主编辑区不受影响。
    let mut clipboard = MockClipboard::with_contents("XYZ");
    // 折叠到行首，让 PASTE 走插入路径而不是替换路径。
    embed_selection = SelectionSet::new(vec![Selection::caret(byte(0))]);
    run_on_focused_field(
        &registry,
        &mut workspace,
        &mut views,
        &mut embed_buffer,
        &mut embed_selection,
        &mut clipboard,
        vec![(editor::PASTE, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(buffer_text(&embed_buffer), "XYZembed");
    assert_eq!(
        text(&workspace, main_buffer_id),
        main_before,
        "PASTE 不应改动主编辑区文本"
    );

    // 3) CUT：剪切的也是内嵌内容，主编辑区文本仍维持原貌。
    let mut clipboard = MockClipboard::new();
    embed_selection = SelectionSet::new(vec![Selection::new(byte(0), byte("XYZ".len()))]);
    run_on_focused_field(
        &registry,
        &mut workspace,
        &mut views,
        &mut embed_buffer,
        &mut embed_selection,
        &mut clipboard,
        vec![(editor::CUT, CommandArgs::new())],
    )
    .unwrap();
    assert_eq!(clipboard.contents(), Some("XYZ"));
    assert_eq!(buffer_text(&embed_buffer), "embed");
    assert_eq!(
        text(&workspace, main_buffer_id),
        main_before,
        "CUT 不应改动主编辑区文本"
    );
}

#[test]
fn editor_default_keymap_should_bind_clipboard_commands() {
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    editor::install(&mut registry, &mut keymap);
    let text_edit = text_edit_context();

    for (chord, expected) in [
        ("mod c", editor::COPY),
        ("mod x", editor::CUT),
        ("mod v", editor::PASTE),
    ] {
        assert_eq!(
            keymap.resolve(&[key(chord)], &text_edit),
            KeymapResolution::Matched {
                command: command_id(expected),
                args: CommandArgs::new(),
            },
            "{chord} 应该绑到 {expected}"
        );
    }
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
        keymap.resolve(&[key("shift tab")], &text_edit),
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
        keymap.resolve(&[key("shift tab")], &multiline),
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
            args: file_tree::FileTreeMoveArgs { delta: -1 }.into(),
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
    let (mut workspace, mut views, _, view_id) = setup("");
    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    file_tree::install(&mut registry, &mut keymap);

    let effects = run_and_collect_effects(
        &registry,
        &mut workspace,
        &mut views,
        view_id,
        vec![
            (
                file_tree::MOVE_SELECTION,
                file_tree::FileTreeMoveArgs { delta: 1 }.into(),
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

    // text_edit 与 text_edit_multiline 的 composition 都是 Inactive（后者只多了 requires_newline 过滤），
    // 同一序列会被同一运行时上下文同时命中——重叠即冲突。
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

#[test]
fn project_picker_esc_routes_through_dismiss_stack() {
    // 接入证明：show_projects_picker 命令 push 一个 dismiss token；
    // picker 上下文里 esc 解析到 DISMISS_TOP；执行 DISMISS_TOP 触发栈顶 invocation（DISMISS），
    // 后者 emit DismissSurface 并清掉残留 token。

    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    zom_command::commands::install_all(&mut registry, &mut keymap);

    let (mut workspace, mut views, _, view_id) = setup("");
    let mut clipboard = MockClipboard::new();
    let mut dismiss = DismissStacks::new();

    // 1) SHOW_PROJECTS_PICKER：栈被 push 一条 token，effect 是 ShowProjectPicker。
    {
        let mut queue = CommandQueue::new();
        queue.enqueue(
            command_id(project_picker::SHOW_PROJECTS_PICKER),
            CommandArgs::new(),
        );
        let mut effects = EffectQueue::new();
        let mut context = CommandContext {
            workspace: &mut workspace,
            views: &mut views,
            active_view_id: Some(view_id),
            focused_field: None,
            queue: &mut queue,
            effects: &mut effects,
            clipboard: &mut clipboard,
            dismiss: &mut dismiss,
            edit_merge_policy: TransactionMergePolicy::Never,
        };
        zom_command::run(&registry, &mut context).unwrap();
        assert_eq!(effects.drain(), vec![HostEffect::ShowProjectPicker]);
        assert_eq!(dismiss.depth(DismissScope::ProjectPicker), 1);
        assert_eq!(
            dismiss.top_label(DismissScope::ProjectPicker),
            Some("关闭项目选择器")
        );
    }

    // 2) picker 上下文里 escape 必须解析到系统级 system.dismiss_top（带 scope=ProjectPicker），
    //    而不再是 picker 自己的 DISMISS。
    let resolution = keymap.resolve(
        &[key("escape")],
        &[KeyContext::project_picker(), KeyContext::global()],
    );
    let (resolved_id, resolved_args) = match resolution {
        KeymapResolution::Matched { command, args } => (command, args),
        other => panic!("escape 应该解析到 system.dismiss_top，实际：{other:?}"),
    };
    assert_eq!(resolved_id, command_id("system.dismiss_top"));
    assert_eq!(resolved_args.get("scope"), Some("ProjectPicker"));

    // 3) 执行 system.dismiss_top(ProjectPicker)：栈被弹空，进而派发 DISMISS，最终 emit DismissSurface。
    {
        let mut queue = CommandQueue::new();
        queue.enqueue(resolved_id.clone(), resolved_args.clone());
        let mut effects = EffectQueue::new();
        let mut context = CommandContext {
            workspace: &mut workspace,
            views: &mut views,
            active_view_id: Some(view_id),
            focused_field: None,
            queue: &mut queue,
            effects: &mut effects,
            clipboard: &mut clipboard,
            dismiss: &mut dismiss,
            edit_merge_policy: TransactionMergePolicy::Never,
        };
        zom_command::run(&registry, &mut context).unwrap();
        assert_eq!(effects.drain(), vec![HostEffect::DismissSurface]);
        assert!(dismiss.is_empty(DismissScope::ProjectPicker));
    }

    // 4) 栈空后再来一次 dismiss_top —— no-op，不产生 effect、不报错。
    {
        let mut queue = CommandQueue::new();
        queue.enqueue(resolved_id, resolved_args);
        let mut effects = EffectQueue::new();
        let mut context = CommandContext {
            workspace: &mut workspace,
            views: &mut views,
            active_view_id: Some(view_id),
            focused_field: None,
            queue: &mut queue,
            effects: &mut effects,
            clipboard: &mut clipboard,
            dismiss: &mut dismiss,
            edit_merge_policy: TransactionMergePolicy::Never,
        };
        zom_command::run(&registry, &mut context).unwrap();
        assert!(effects.drain().is_empty());
    }
}

#[test]
fn project_picker_show_is_idempotent_does_not_stack_tokens() {
    // 已开 picker 时再调 SHOW_PROJECTS_PICKER（host 二次响应同一个快捷键），不应该让栈累积。

    let mut registry = CommandRegistry::new();
    let mut keymap = Keymap::new();
    zom_command::commands::install_all(&mut registry, &mut keymap);

    let (mut workspace, mut views, _, view_id) = setup("");
    let mut clipboard = MockClipboard::new();
    let mut dismiss = DismissStacks::new();

    let mut queue = CommandQueue::new();
    queue.enqueue(
        command_id(project_picker::SHOW_PROJECTS_PICKER),
        CommandArgs::new(),
    );
    queue.enqueue(
        command_id(project_picker::SHOW_PROJECTS_PICKER),
        CommandArgs::new(),
    );
    let mut effects = EffectQueue::new();
    let mut context = CommandContext {
        workspace: &mut workspace,
        views: &mut views,
        active_view_id: Some(view_id),
        focused_field: None,
        queue: &mut queue,
        effects: &mut effects,
        clipboard: &mut clipboard,
        dismiss: &mut dismiss,
        edit_merge_policy: TransactionMergePolicy::Never,
    };
    zom_command::run(&registry, &mut context).unwrap();
    assert_eq!(dismiss.depth(DismissScope::ProjectPicker), 1);
}
