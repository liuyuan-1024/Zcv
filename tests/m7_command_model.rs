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
        Command::MoveSubwordLeft { extend: true },
        Command::MoveSubwordRight { extend: false },
        Command::MoveSymbolLeft { extend: true },
        Command::MoveSymbolRight { extend: false },
    ];

    assert!(matches!(commands[0], Command::MoveLeft { extend: false }));
    assert!(matches!(commands[1], Command::MoveRight { extend: true }));
    assert!(matches!(commands[4], Command::MoveLineStart { extend: true }));
    assert!(matches!(commands[10], Command::MoveSymbolLeft { extend: true }));
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
    assert_eq!(add, Command::AddSelection(Selection::caret(CharOffset::new(8))));

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
fn command_source_maps_to_transaction_source() {
    assert_eq!(
        TransactionSource::from(CommandSource::Keyboard),
        TransactionSource::Keyboard
    );
    assert_eq!(
        TransactionSource::from(CommandSource::Mouse),
        TransactionSource::Mouse
    );
    assert_eq!(
        TransactionSource::from(CommandSource::Paste),
        TransactionSource::Paste
    );
    assert_eq!(
        TransactionSource::from(CommandSource::CommandPalette),
        TransactionSource::Command
    );
    assert_eq!(
        TransactionSource::from(CommandSource::Menu),
        TransactionSource::Command
    );
    assert_eq!(
        TransactionSource::from(CommandSource::Ime),
        TransactionSource::Composition
    );
    assert_eq!(
        TransactionSource::from(CommandSource::Macro),
        TransactionSource::Macro
    );
    assert_eq!(
        TransactionSource::from(CommandSource::External),
        TransactionSource::External
    );
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
fn command_context_builds_transaction_metadata_for_command() {
    let context = CommandContext::paste();
    let metadata = context.transaction_metadata_for(&Command::insert_text("hello"));

    assert_eq!(metadata.source, TransactionSource::Paste);
    assert_eq!(metadata.description.as_deref(), Some("insert text"));
    assert!(metadata.record_history);
}

#[test]
fn command_specific_metadata_overrides_generic_source_when_needed() {
    let keyboard = CommandContext::keyboard();
    let delete_metadata = keyboard.transaction_metadata_for(&Command::DeleteBackward);
    let undo_metadata = keyboard.transaction_metadata_for(&Command::Undo);
    let composition_metadata = CommandContext::ime()
        .transaction_metadata_for(&Command::composition_commit("你"));

    assert_eq!(delete_metadata.source, TransactionSource::Delete);
    assert_eq!(delete_metadata.description.as_deref(), Some("delete backward"));
    assert_eq!(undo_metadata.source, TransactionSource::Undo);
    assert_eq!(undo_metadata.description.as_deref(), Some("undo"));
    assert_eq!(composition_metadata.source, TransactionSource::Composition);
    assert_eq!(
        composition_metadata.description.as_deref(),
        Some("composition commit")
    );
}

#[test]
fn command_description_is_stable_and_short() {
    assert_eq!(Command::insert_text("very long text").description(), "insert text");
    assert_eq!(
        (Command::MoveWordRight { extend: true }).description(),
        "extend word right"
    );
    assert_eq!((Command::SelectAll).description(), "select all");
    assert_eq!((Command::Redo).description(), "redo");
}
