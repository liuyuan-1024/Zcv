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
