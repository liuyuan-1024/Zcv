use std::time::{Duration, UNIX_EPOCH};

use zom_engine::*;

#[test]
fn command_enum_supports_text_input_commands() {
    let insert = Command::insert_text("hello");
    assert_eq!(insert, Command::InsertText("hello".to_string()));

    let replace = Command::replace_selections("world");
    assert_eq!(replace, Command::ReplaceSelections("world".to_string()));

    assert_eq!(Command::DeleteSelection, Command::DeleteSelection);
}

#[test]
fn command_enum_supports_delete_commands() {
    let commands = [
        Command::DeleteBackward,
        Command::DeleteForward,
        Command::DeleteWordBackward,
        Command::DeleteWordForward,
        Command::DeleteSubwordBackward,
        Command::DeleteSubwordForward,
    ];

    for command in commands {
        let cloned = command.clone();
        assert_eq!(command, cloned);
    }
}

#[test]
fn command_enum_supports_movement_commands_with_extend_flag() {
    let commands = [
        Command::MoveLeft { extend: false },
        Command::MoveRight { extend: true },
        Command::MoveUp { extend: false },
        Command::MoveDown { extend: true },
        Command::MoveLineStart { extend: true },
        Command::MoveLineEnd { extend: false },
        Command::MoveWordLeft { extend: true },
        Command::MoveWordRight { extend: false },
        Command::MoveIdentifierLeft { extend: true },
        Command::MoveIdentifierRight { extend: false },
        Command::MoveSubwordLeft { extend: true },
        Command::MoveSubwordRight { extend: false },
        Command::MoveSymbolLeft { extend: true },
        Command::MoveSymbolRight { extend: false },
    ];

    assert!(matches!(commands[0], Command::MoveLeft { extend: false }));
    assert!(matches!(commands[1], Command::MoveRight { extend: true }));
    assert!(matches!(
        commands[4],
        Command::MoveLineStart { extend: true }
    ));
    assert!(matches!(
        commands[10],
        Command::MoveSubwordLeft { extend: true }
    ));
    assert!(matches!(
        commands[12],
        Command::MoveSymbolLeft { extend: true }
    ));
}

#[test]
fn command_enum_supports_selection_commands() {
    let selections = SelectionSet::new(vec![
        Selection::caret(CharOffset::new(1)),
        Selection::new(CharOffset::new(3), CharOffset::new(5)),
    ]);

    let set = Command::SetSelections(selections.clone());
    assert_eq!(set, Command::SetSelections(selections));

    let add = Command::AddSelection(Selection::caret(CharOffset::new(8)));
    assert_eq!(
        add,
        Command::AddSelection(Selection::caret(CharOffset::new(8)))
    );

    assert_eq!(Command::SelectAll, Command::SelectAll);
    assert_eq!(Command::ClearSelections, Command::ClearSelections);
}

#[test]
fn command_enum_supports_history_and_composition_commands() {
    assert_eq!(Command::Undo, Command::Undo);
    assert_eq!(Command::Redo, Command::Redo);
    assert_eq!(Command::CompositionStart, Command::CompositionStart);
    assert_eq!(
        Command::composition_update("ni"),
        Command::CompositionUpdate("ni".to_string())
    );
    assert_eq!(
        Command::composition_commit("你"),
        Command::CompositionCommit("你".to_string())
    );
    assert_eq!(Command::CompositionCancel, Command::CompositionCancel);
}

#[test]
fn command_sources_are_data_model_only() {
    let sources = [
        CommandSource::Keyboard,
        CommandSource::Mouse,
        CommandSource::Paste,
        CommandSource::CommandPalette,
        CommandSource::Menu,
        CommandSource::Ime,
        CommandSource::Macro,
        CommandSource::External,
    ];

    for source in sources {
        let context = CommandContext::new(source);
        assert_eq!(context.source(), source);
    }
}

#[test]
fn command_context_defaults_to_keyboard_single_repeat_without_timestamp() {
    let context = CommandContext::default();

    assert_eq!(context.source(), CommandSource::Keyboard);
    assert_eq!(context.repeat(), CommandRepeat::DEFAULT);
    assert_eq!(context.repeat_count(), 1);
    assert_eq!(context.timestamp(), None);
}

#[test]
fn command_context_supports_source_repeat_and_timestamp() {
    let timestamp = UNIX_EPOCH + Duration::from_millis(42);
    let context = CommandContext::command_palette()
        .with_repeat_count(3)
        .expect("repeat count > 0")
        .with_timestamp(timestamp);

    assert_eq!(context.source(), CommandSource::CommandPalette);
    assert_eq!(context.repeat_count(), 3);
    assert_eq!(context.timestamp(), Some(timestamp));
    assert_eq!(CommandRepeat::new(0), None);
}

#[test]
fn command_description_is_stable_and_short() {
    assert_eq!(
        Command::insert_text("very long text").description(),
        "insert text"
    );
    assert_eq!(
        (Command::MoveWordRight { extend: true }).description(),
        "extend word right"
    );
    assert_eq!(
        (Command::MoveIdentifierLeft { extend: false }).description(),
        "move identifier left"
    );
    assert_eq!((Command::SelectAll).description(), "select all");
    assert_eq!((Command::Redo).description(), "redo");
}

fn test_buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).expect("buffer")
}

#[test]
fn execute_insert_text_command_creates_transaction_and_outcome() {
    let mut buffer = test_buffer("hello");
    buffer
        .set_selection(SelectionSet::caret(CharOffset::new(5)))
        .expect("selection");

    let outcome = buffer
        .execute_command(Command::insert_text("!"), CommandContext::paste())
        .expect("execute insert text");

    assert_eq!(buffer.text().as_ref(), "hello!");
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(6));
    assert_eq!(outcome.old_version(), BufferVersion::INITIAL);
    assert_eq!(outcome.new_version(), buffer.version());
    assert_eq!(outcome.transaction_id(), None);
    assert!(outcome.text_changed());
    assert!(outcome.selection_changed());
    assert!(!outcome.composition_changed());
    assert_eq!(outcome.description(), "insert text");
    assert!(buffer.can_undo());
}

#[test]
fn execute_movement_command_updates_selection_without_text_transaction() {
    let mut buffer = test_buffer("abc");
    buffer
        .set_selection(SelectionSet::caret(CharOffset::new(1)))
        .expect("selection");
    let version = buffer.version();

    let outcome = buffer
        .execute_command(
            Command::MoveRight { extend: true },
            CommandContext::keyboard(),
        )
        .expect("execute move");

    assert_eq!(buffer.text().as_ref(), "abc");
    assert_eq!(buffer.version(), version);
    assert_eq!(buffer.selection().primary().anchor(), CharOffset::new(1));
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(2));
    assert!(!outcome.text_changed());
    assert!(outcome.selection_changed());
    assert!(!buffer.can_undo());
}

#[test]
fn execute_line_movement_commands_use_logical_line_boundaries() {
    let mut buffer = test_buffer("ab\ncdef\ng");
    buffer
        .set_selection(SelectionSet::caret(CharOffset::new(5)))
        .expect("selection");

    buffer
        .execute_command(
            Command::MoveLineStart { extend: false },
            CommandContext::keyboard(),
        )
        .expect("line start");
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(3));

    buffer
        .execute_command(
            Command::MoveLineEnd { extend: false },
            CommandContext::keyboard(),
        )
        .expect("line end");
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(7));

    buffer
        .execute_command(
            Command::MoveDown { extend: false },
            CommandContext::keyboard(),
        )
        .expect("move down clamps to short line");
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(9));
}

#[test]
fn execute_delete_word_command_uses_existing_movement_boundaries() {
    let mut buffer = test_buffer("hello world");
    buffer
        .set_selection(SelectionSet::caret(CharOffset::new(11)))
        .expect("selection");

    let outcome = buffer
        .execute_command(Command::DeleteWordBackward, CommandContext::keyboard())
        .expect("delete word backward");

    assert_eq!(buffer.text().as_ref(), "hello ");
    assert!(outcome.text_changed());
    assert_eq!(outcome.description(), "delete word backward");
}

#[test]
fn execute_selection_commands_only_change_selection() {
    let mut buffer = test_buffer("abc");

    let select_all = buffer
        .execute_command(Command::SelectAll, CommandContext::command_palette())
        .expect("select all");
    assert_eq!(
        buffer.selection().primary().range(),
        TextRange::new(CharOffset::ZERO, CharOffset::new(3)).unwrap()
    );
    assert!(!select_all.text_changed());
    assert!(select_all.selection_changed());

    let clear = buffer
        .execute_command(Command::ClearSelections, CommandContext::keyboard())
        .expect("clear selections");
    assert_eq!(
        buffer.selection().primary(),
        &Selection::caret(CharOffset::new(3))
    );
    assert!(!clear.text_changed());
    assert!(clear.selection_changed());
}

#[test]
fn execute_undo_redo_commands_use_history_system() {
    let mut buffer = test_buffer("a");
    buffer
        .set_selection(SelectionSet::caret(CharOffset::new(1)))
        .expect("selection");
    buffer
        .execute_command(Command::insert_text("b"), CommandContext::keyboard())
        .expect("insert");
    assert_eq!(buffer.text().as_ref(), "ab");

    let undo = buffer
        .execute_command(Command::Undo, CommandContext::keyboard())
        .expect("undo");
    assert_eq!(buffer.text().as_ref(), "a");
    assert!(undo.text_changed());
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(1));

    let redo = buffer
        .execute_command(Command::Redo, CommandContext::keyboard())
        .expect("redo");
    assert_eq!(buffer.text().as_ref(), "ab");
    assert!(redo.text_changed());
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(2));
}

#[test]
fn execute_ime_commands_reuse_composition_pipeline() {
    let mut buffer = test_buffer("");

    let start = buffer
        .execute_command(Command::CompositionStart, CommandContext::ime())
        .expect("composition start");
    assert!(buffer.is_composing());
    assert!(!start.text_changed());
    assert!(start.composition_changed());

    let update = buffer
        .execute_command(Command::composition_update("ni"), CommandContext::ime())
        .expect("composition update");
    assert_eq!(buffer.text().as_ref(), "ni");
    assert!(buffer.is_composing());
    assert!(update.text_changed());
    assert!(update.composition_changed());
    assert!(!buffer.can_undo());

    let commit = buffer
        .execute_command(Command::composition_commit("你"), CommandContext::ime())
        .expect("composition commit");
    assert_eq!(buffer.text().as_ref(), "你");
    assert!(!buffer.is_composing());
    assert!(commit.text_changed());
    assert!(commit.composition_changed());
    assert!(buffer.can_undo());

    buffer
        .execute_command(Command::Undo, CommandContext::keyboard())
        .expect("undo committed composition");
    assert_eq!(buffer.text().as_ref(), "");
}

#[test]
fn m7d_insert_text_command_is_equivalent_to_existing_multi_cursor_insert() {
    let selections = SelectionSet::new(vec![
        Selection::caret(CharOffset::new(1)),
        Selection::caret(CharOffset::new(3)),
        Selection::new(CharOffset::new(5), CharOffset::new(6)),
    ]);

    let mut command_buffer = test_buffer("abcdef");
    command_buffer
        .set_selection(selections.clone())
        .expect("selection");
    command_buffer
        .execute_command(Command::insert_text("X"), CommandContext::keyboard())
        .expect("command insert");

    let mut direct_buffer = test_buffer("abcdef");
    direct_buffer
        .insert_at_selections(selections, "X")
        .expect("direct insert");

    assert_eq!(command_buffer.text(), direct_buffer.text());
    assert_eq!(command_buffer.selection(), direct_buffer.selection());
    assert!(command_buffer.can_undo());
}

#[test]
fn m7d_replace_selections_command_is_equivalent_to_existing_multi_selection_replace() {
    let selections = SelectionSet::new(vec![
        Selection::new(CharOffset::new(0), CharOffset::new(2)),
        Selection::new(CharOffset::new(4), CharOffset::new(6)),
    ]);

    let mut command_buffer = test_buffer("abcdef");
    command_buffer
        .set_selection(selections.clone())
        .expect("selection");
    command_buffer
        .execute_command(
            Command::replace_selections("Z"),
            CommandContext::command_palette(),
        )
        .expect("command replace");

    let mut direct_buffer = test_buffer("abcdef");
    direct_buffer
        .replace_selections(selections, "Z")
        .expect("direct replace");

    assert_eq!(command_buffer.text(), direct_buffer.text());
    assert_eq!(command_buffer.selection(), direct_buffer.selection());
    assert!(command_buffer.can_undo());
}

#[test]
fn m7d_delete_backward_and_forward_are_equivalent_to_existing_multi_cursor_delete() {
    let backward_selections = SelectionSet::new(vec![
        Selection::caret(CharOffset::new(2)),
        Selection::new(CharOffset::new(4), CharOffset::new(6)),
    ]);

    let mut command_backward = test_buffer("abcdef");
    command_backward
        .set_selection(backward_selections.clone())
        .expect("selection");
    command_backward
        .execute_command(Command::DeleteBackward, CommandContext::keyboard())
        .expect("command delete backward");

    let mut direct_backward = test_buffer("abcdef");
    direct_backward
        .delete_backward_at_selections(backward_selections)
        .expect("direct delete backward");

    assert_eq!(command_backward.text(), direct_backward.text());
    assert_eq!(command_backward.selection(), direct_backward.selection());

    let forward_selections = SelectionSet::new(vec![
        Selection::caret(CharOffset::new(1)),
        Selection::new(CharOffset::new(4), CharOffset::new(6)),
    ]);

    let mut command_forward = test_buffer("abcdef");
    command_forward
        .set_selection(forward_selections.clone())
        .expect("selection");
    command_forward
        .execute_command(Command::DeleteForward, CommandContext::keyboard())
        .expect("command delete forward");

    let mut direct_forward = test_buffer("abcdef");
    direct_forward
        .delete_forward_at_selections(forward_selections)
        .expect("direct delete forward");

    assert_eq!(command_forward.text(), direct_forward.text());
    assert_eq!(command_forward.selection(), direct_forward.selection());
}

#[test]
fn m7d_word_identifier_subword_and_symbol_movement_commands_reuse_m6b_movement_policy() {
    let cases = [
        (
            Command::MoveWordRight { extend: false },
            MovementDirection::Next,
            MovementUnit::Word,
            false,
            CharOffset::new(0),
        ),
        (
            Command::MoveWordLeft { extend: true },
            MovementDirection::Previous,
            MovementUnit::Word,
            true,
            CharOffset::new(9),
        ),
        (
            Command::MoveIdentifierRight { extend: false },
            MovementDirection::Next,
            MovementUnit::Identifier,
            false,
            CharOffset::new(0),
        ),
        (
            Command::MoveIdentifierLeft { extend: true },
            MovementDirection::Previous,
            MovementUnit::Identifier,
            true,
            CharOffset::new(12),
        ),
        (
            Command::MoveSubwordRight { extend: false },
            MovementDirection::Next,
            MovementUnit::Subword,
            false,
            CharOffset::new(0),
        ),
        (
            Command::MoveSubwordLeft { extend: true },
            MovementDirection::Previous,
            MovementUnit::Subword,
            true,
            CharOffset::new(7),
        ),
        (
            Command::MoveSymbolRight { extend: false },
            MovementDirection::Next,
            MovementUnit::Symbol,
            false,
            CharOffset::new(8),
        ),
        (
            Command::MoveSymbolLeft { extend: true },
            MovementDirection::Previous,
            MovementUnit::Symbol,
            true,
            CharOffset::new(10),
        ),
    ];

    for (command, direction, unit, extend, start) in cases {
        let text = "fooBar + baz_qux";
        let mut command_buffer = test_buffer(text);
        command_buffer
            .set_selection(SelectionSet::caret(start))
            .expect("selection");
        let command_version = command_buffer.version();
        command_buffer
            .execute_command(command, CommandContext::keyboard())
            .expect("command movement");

        let mut direct_buffer = test_buffer(text);
        direct_buffer
            .set_selection(SelectionSet::caret(start))
            .expect("selection");
        let direct_version = direct_buffer.version();
        direct_buffer
            .move_current_selection(direction, unit, extend)
            .expect("direct movement");

        assert_eq!(command_buffer.selection(), direct_buffer.selection());
        assert_eq!(command_buffer.version(), command_version);
        assert_eq!(direct_buffer.version(), direct_version);
        assert!(!command_buffer.can_undo());
    }
}

#[test]
fn m7d_composition_commands_reuse_m6c_composition_pipeline() {
    let mut command_buffer = test_buffer("");
    command_buffer
        .execute_command(Command::CompositionStart, CommandContext::ime())
        .expect("command composition start");
    command_buffer
        .execute_command(Command::composition_update("ni"), CommandContext::ime())
        .expect("command composition update");
    command_buffer
        .execute_command(Command::composition_commit("你"), CommandContext::ime())
        .expect("command composition commit");

    let mut direct_buffer = test_buffer("");
    direct_buffer
        .start_composition()
        .expect("direct composition start");
    direct_buffer
        .update_composition("ni", None)
        .expect("direct composition update");
    direct_buffer
        .commit_composition("你")
        .expect("direct composition commit");

    assert_eq!(command_buffer.text(), direct_buffer.text());
    assert_eq!(command_buffer.selection(), direct_buffer.selection());
    assert_eq!(command_buffer.is_composing(), direct_buffer.is_composing());
    assert_eq!(command_buffer.can_undo(), direct_buffer.can_undo());
}

#[test]
fn m7d_undo_redo_commands_restore_text_and_selection_set() {
    let before = SelectionSet::new(vec![
        Selection::caret(CharOffset::new(1)),
        Selection::caret(CharOffset::new(3)),
    ]);

    let mut buffer = test_buffer("abcd");
    buffer.set_selection(before.clone()).expect("selection");
    buffer
        .execute_command(Command::insert_text("X"), CommandContext::keyboard())
        .expect("insert");
    let after_insert_text = buffer.text().into_owned();
    let after_insert_selection = buffer.selection().clone();

    buffer
        .execute_command(Command::Undo, CommandContext::keyboard())
        .expect("undo");
    assert_eq!(buffer.text().as_ref(), "abcd");
    assert_eq!(buffer.selection(), &before);

    buffer
        .execute_command(Command::Redo, CommandContext::keyboard())
        .expect("redo");
    assert_eq!(buffer.text().as_ref(), after_insert_text.as_str());
    assert_eq!(buffer.selection(), &after_insert_selection);
}

#[test]
fn m7d_command_executor_does_not_bypass_transaction_selection_or_composition_pipelines() {
    let mut buffer = test_buffer("abc");
    buffer
        .set_selection(SelectionSet::caret(CharOffset::new(1)))
        .expect("selection");

    let original_version = buffer.version();
    let insert = buffer
        .execute_command(Command::insert_text("X"), CommandContext::keyboard())
        .expect("insert");
    assert!(insert.text_changed());
    assert!(buffer.version() != original_version);
    assert!(buffer.can_undo());

    let version_after_insert = buffer.version();
    let move_outcome = buffer
        .execute_command(
            Command::MoveRight { extend: true },
            CommandContext::keyboard(),
        )
        .expect("move");
    assert!(!move_outcome.text_changed());
    assert!(move_outcome.selection_changed());
    assert_eq!(buffer.version(), version_after_insert);

    let composition_update = buffer
        .execute_command(Command::composition_update("ni"), CommandContext::ime())
        .expect("composition update");
    assert!(composition_update.text_changed());
    assert!(buffer.is_composing());
    assert!(buffer.can_undo());

    let composition_commit = buffer
        .execute_command(Command::composition_commit("你"), CommandContext::ime())
        .expect("composition commit");
    assert!(composition_commit.text_changed());
    assert!(!buffer.is_composing());

    buffer
        .execute_command(Command::Undo, CommandContext::keyboard())
        .expect("undo composition commit");
    assert!(!buffer.is_composing());
}

#[test]
fn repeat_count_repeats_repeatable_text_and_movement_commands() {
    let mut text_buffer = test_buffer("");
    text_buffer
        .execute_command(
            Command::insert_text("x"),
            CommandContext::keyboard()
                .with_repeat_count(3)
                .expect("repeat count"),
        )
        .expect("repeat insert");
    assert_eq!(text_buffer.text().as_ref(), "xxx");

    let mut move_buffer = test_buffer("abcd");
    move_buffer
        .execute_command(
            Command::MoveRight { extend: false },
            CommandContext::keyboard()
                .with_repeat_count(3)
                .expect("repeat count"),
        )
        .expect("repeat move");
    assert_eq!(
        move_buffer.selection().primary(),
        &Selection::caret(CharOffset::new(3))
    );
}

#[test]
fn repeat_count_is_ignored_for_absolute_selection_and_composition_commands() {
    let mut buffer = test_buffer("abcd");
    buffer
        .set_selection(SelectionSet::caret(CharOffset::new(1)))
        .expect("selection");

    buffer
        .execute_command(
            Command::AddSelection(Selection::caret(CharOffset::new(3))),
            CommandContext::keyboard()
                .with_repeat_count(3)
                .expect("repeat count"),
        )
        .expect("add selection once");
    assert_eq!(buffer.selection().len(), 2);

    let mut ime_buffer = test_buffer("");
    ime_buffer
        .execute_command(
            Command::CompositionStart,
            CommandContext::ime()
                .with_repeat_count(3)
                .expect("repeat count"),
        )
        .expect("composition start once");
    assert!(ime_buffer.is_composing());
}
