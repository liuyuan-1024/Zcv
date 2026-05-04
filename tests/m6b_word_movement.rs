use zom_engine::{
    Buffer, BufferConfig, CharOffset, MovementDirection, MovementUnit, Selection, SelectionSet,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

#[test]
fn word_movement_uses_unicode_word_boundaries() {
    let buffer = buffer("hello 世界 👋");

    assert_eq!(buffer.next_word_boundary(c(0)).unwrap(), c(5));
    assert_eq!(buffer.next_word_boundary(c(5)).unwrap(), c(6));
    assert_eq!(buffer.next_word_boundary(c(6)).unwrap(), c(8));

    assert_eq!(buffer.previous_word_boundary(c(8)).unwrap(), c(6));
    assert_eq!(buffer.previous_word_boundary(c(6)).unwrap(), c(5));
    assert_eq!(buffer.previous_word_boundary(c(5)).unwrap(), c(0));
}

#[test]
fn identifier_movement_keeps_snake_case_and_dollar_identifiers_together() {
    let buffer = buffer("let foo_bar = baz$qux;");

    assert_eq!(buffer.next_identifier_boundary(c(0)).unwrap(), c(3));
    assert_eq!(buffer.next_identifier_boundary(c(3)).unwrap(), c(4));
    assert_eq!(buffer.next_identifier_boundary(c(4)).unwrap(), c(11));
    assert_eq!(buffer.next_identifier_boundary(c(14)).unwrap(), c(21));

    assert_eq!(buffer.previous_identifier_boundary(c(21)).unwrap(), c(14));
    assert_eq!(buffer.previous_identifier_boundary(c(14)).unwrap(), c(11));
    assert_eq!(buffer.previous_identifier_boundary(c(11)).unwrap(), c(4));
}

#[test]
fn subword_movement_splits_snake_camel_pascal_and_digits() {
    let buffer = buffer("foo_barBaz42 HTTPServer");

    assert_eq!(buffer.next_subword_boundary(c(0)).unwrap(), c(3));
    assert_eq!(buffer.next_subword_boundary(c(3)).unwrap(), c(4));
    assert_eq!(buffer.next_subword_boundary(c(4)).unwrap(), c(7));
    assert_eq!(buffer.next_subword_boundary(c(7)).unwrap(), c(10));
    assert_eq!(buffer.next_subword_boundary(c(10)).unwrap(), c(12));

    assert_eq!(buffer.next_subword_boundary(c(13)).unwrap(), c(17));
    assert_eq!(buffer.next_subword_boundary(c(17)).unwrap(), c(23));

    assert_eq!(buffer.previous_subword_boundary(c(23)).unwrap(), c(17));
    assert_eq!(buffer.previous_subword_boundary(c(17)).unwrap(), c(13));
    assert_eq!(buffer.previous_subword_boundary(c(10)).unwrap(), c(7));
    assert_eq!(buffer.previous_subword_boundary(c(7)).unwrap(), c(4));
}

#[test]
fn symbol_movement_targets_operator_and_punctuation_runs() {
    let buffer = buffer("foo::bar += baz");

    assert_eq!(buffer.next_symbol_boundary(c(0)).unwrap(), c(3));
    assert_eq!(buffer.next_symbol_boundary(c(3)).unwrap(), c(5));
    assert_eq!(buffer.next_symbol_boundary(c(5)).unwrap(), c(9));
    assert_eq!(buffer.next_symbol_boundary(c(9)).unwrap(), c(11));

    assert_eq!(buffer.previous_symbol_boundary(c(11)).unwrap(), c(9));
    assert_eq!(buffer.previous_symbol_boundary(c(9)).unwrap(), c(5));
    assert_eq!(buffer.previous_symbol_boundary(c(5)).unwrap(), c(3));
}

#[test]
fn generic_movement_boundary_dispatches_by_unit() {
    let buffer = buffer("foo_bar");

    assert_eq!(
        buffer
            .movement_boundary(c(0), MovementDirection::Next, MovementUnit::Identifier)
            .unwrap(),
        c(7)
    );
    assert_eq!(
        buffer
            .movement_boundary(c(0), MovementDirection::Next, MovementUnit::Subword)
            .unwrap(),
        c(3)
    );
}

#[test]
fn grapheme_movement_dispatches_to_existing_grapheme_boundaries() {
    let buffer = buffer("a🇨🇳b");

    assert_eq!(
        buffer
            .movement_boundary(c(1), MovementDirection::Next, MovementUnit::Grapheme)
            .unwrap(),
        c(3)
    );
    assert_eq!(
        buffer
            .movement_boundary(c(3), MovementDirection::Previous, MovementUnit::Grapheme)
            .unwrap(),
        c(1)
    );
}

#[test]
fn move_current_selection_without_extend_collapses_to_new_caret() {
    let mut buffer = buffer("hello world");
    buffer.set_selection(SelectionSet::caret(c(0))).unwrap();

    let moved = buffer
        .move_current_selection(MovementDirection::Next, MovementUnit::Word, false)
        .unwrap();

    assert_eq!(moved.as_slice(), &[Selection::caret(c(5))]);
    assert_eq!(buffer.selection().as_slice(), &[Selection::caret(c(5))]);
}

#[test]
fn move_current_selection_with_extend_preserves_anchor() {
    let mut buffer = buffer("hello world");
    buffer.set_selection(SelectionSet::caret(c(0))).unwrap();

    let moved = buffer
        .move_current_selection(MovementDirection::Next, MovementUnit::Word, true)
        .unwrap();

    assert_eq!(moved.as_slice(), &[Selection::new(c(0), c(5))]);
    assert_eq!(buffer.selection().as_slice(), &[Selection::new(c(0), c(5))]);
}

#[test]
fn move_selections_supports_multi_cursor_heads() {
    let mut buffer = buffer("one two three four");
    let selections = SelectionSet::new(vec![Selection::caret(c(0)), Selection::caret(c(8))]);

    let moved = buffer
        .move_selections(selections, MovementDirection::Next, MovementUnit::Word, false)
        .unwrap();

    assert_eq!(
        moved.as_slice(),
        &[Selection::caret(c(3)), Selection::caret(c(13))]
    );
}

#[test]
fn movement_rejects_out_of_bounds_offsets() {
    let buffer = buffer("abc");

    assert!(
        buffer
            .next_word_boundary(CharOffset::new(4))
            .unwrap_err()
            .to_string()
            .contains("越界")
    );
}
